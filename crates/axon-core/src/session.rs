//! One conversation: a task that owns the history and runs a turn per prompt.

use std::pin::pin;

use anyhow::Result;
use async_openai::types::chat::ChatCompletionRequestMessage;
use axon_protocol::{Command, Event, SessionHandle};
use futures::StreamExt;
use tokio::sync::mpsc;

use crate::model::ModelClient;

const COMMAND_BUFFER: usize = 16;
const EVENT_BUFFER: usize = 64;

/// Start a session task and return the frontend's end of it.
pub(crate) fn spawn(client: ModelClient) -> SessionHandle {
    let (commands, command_rx) = mpsc::channel(COMMAND_BUFFER);
    let (event_tx, events) = mpsc::channel(EVENT_BUFFER);
    tokio::spawn(run(client, command_rx, event_tx));
    SessionHandle { commands, events }
}

/// Runs until the frontend drops its command sender or its event receiver.
async fn run(
    client: ModelClient,
    mut commands: mpsc::Receiver<Command>,
    events: mpsc::Sender<Event>,
) {
    let mut history: Vec<ChatCompletionRequestMessage> = Vec::new();

    while let Some(command) = commands.recv().await {
        match command {
            Command::Prompt { text } => {
                history.push(ChatCompletionRequestMessage::User(text.into()));
                match turn(&client, &history, &events).await {
                    Ok(reply) => {
                        history.push(ChatCompletionRequestMessage::Assistant(reply.into()))
                    }
                    Err(e) => {
                        history.pop();
                        let message = format!("{e:#}");
                        if events.send(Event::Error { message }).await.is_err() {
                            return;
                        }
                    }
                }
                if events.send(Event::TurnEnded).await.is_err() {
                    return;
                }
            }
        }
    }
}

/// Stream one model reply to the frontend and return its full text.
async fn turn(
    client: &ModelClient,
    history: &[ChatCompletionRequestMessage],
    events: &mpsc::Sender<Event>,
) -> Result<String> {
    let mut stream = pin!(client.stream_text(history.to_vec()).await?);
    let mut reply = String::new();
    while let Some(delta) = stream.next().await {
        let text = delta?;
        reply.push_str(&text);
        events.send(Event::TextDelta { text }).await?;
    }
    Ok(reply)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{api_url, client_error, config, reply, respond};
    use serde_json::{Value, json};
    use std::time::Duration;
    use wiremock::MockServer;

    fn session(server: &MockServer) -> SessionHandle {
        spawn(ModelClient::new(&config(api_url(server))))
    }

    /// Send a prompt and collect events up to and including `TurnEnded`.
    async fn prompt(handle: &mut SessionHandle, text: &str) -> Vec<Event> {
        handle
            .commands
            .send(Command::Prompt { text: text.into() })
            .await
            .unwrap();
        let mut events = Vec::new();
        while let Some(event) = handle.events.recv().await {
            let done = event == Event::TurnEnded;
            events.push(event);
            if done {
                break;
            }
        }
        events
    }

    /// The `messages` of each request the server received, in order.
    async fn sent_messages(server: &MockServer) -> Vec<Value> {
        server
            .received_requests()
            .await
            .unwrap()
            .iter()
            .map(|request| request.body_json::<Value>().unwrap()["messages"].clone())
            .collect()
    }

    fn delta(text: &str) -> Event {
        Event::TextDelta { text: text.into() }
    }

    #[tokio::test]
    async fn prompt_streams_deltas_then_ends_the_turn() {
        let server = MockServer::start().await;
        respond(&server, reply(&["Hello", ", world"])).await;
        let mut handle = session(&server);

        assert_eq!(
            prompt(&mut handle, "hi").await,
            [delta("Hello"), delta(", world"), Event::TurnEnded]
        );
    }

    #[tokio::test]
    async fn failed_turn_reports_an_error_and_is_left_out_of_history() {
        let server = MockServer::start().await;
        respond(&server, client_error()).await;
        respond(&server, reply(&["ok"])).await;
        let mut handle = session(&server);

        let events = prompt(&mut handle, "first").await;
        assert!(
            matches!(events[..], [Event::Error { .. }, Event::TurnEnded]),
            "{events:?}"
        );

        assert_eq!(
            prompt(&mut handle, "second").await,
            [delta("ok"), Event::TurnEnded]
        );
        assert_eq!(
            sent_messages(&server).await[1],
            json!([{"role": "user", "content": "second"}])
        );
    }

    #[tokio::test]
    async fn next_prompt_sends_the_previous_exchange() {
        let server = MockServer::start().await;
        respond(&server, reply(&["Hel", "lo"])).await;
        respond(&server, reply(&["Bye"])).await;
        let mut handle = session(&server);

        prompt(&mut handle, "hi").await;
        prompt(&mut handle, "bye").await;

        assert_eq!(
            sent_messages(&server).await[1],
            json!([
                {"role": "user", "content": "hi"},
                {"role": "assistant", "content": "Hello"},
                {"role": "user", "content": "bye"}
            ])
        );
    }

    #[tokio::test]
    async fn session_stops_when_the_event_receiver_is_dropped() {
        let server = MockServer::start().await;
        respond(&server, reply(&["Hello"])).await;
        let SessionHandle { commands, events } = session(&server);
        drop(events);

        commands
            .send(Command::Prompt { text: "hi".into() })
            .await
            .unwrap();

        tokio::time::timeout(Duration::from_secs(5), commands.closed())
            .await
            .expect("session should stop");
    }
}
