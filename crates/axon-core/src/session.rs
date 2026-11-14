//! One conversation: a task that owns the history and runs a turn per prompt.

use std::pin::pin;

use anyhow::Result;
use async_openai::types::chat::ChatCompletionRequestMessage;
use axon_protocol::{Command, Event, SessionHandle};
use futures::StreamExt;
use tokio::sync::mpsc;

use crate::model::ModelClient;

/// Start a session task and return the frontend's end of it.
pub(crate) fn spawn(client: ModelClient) -> SessionHandle {
    let (commands, command_rx) = mpsc::channel(16);
    let (event_tx, events) = mpsc::channel(64);
    tokio::spawn(run(client, command_rx, event_tx));
    SessionHandle { commands, events }
}

/// Runs until the frontend drops its command sender.
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
                        let _ = events
                            .send(Event::Error {
                                message: format!("{e:#}"),
                            })
                            .await;
                    }
                }
                let _ = events.send(Event::TurnEnded).await;
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
        let _ = events.send(Event::TextDelta { text }).await;
    }
    Ok(reply)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::ModelConfig;
    use serde_json::{Value, json};
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    /// A 200 SSE response streaming these text deltas.
    fn reply(deltas: &[&str]) -> ResponseTemplate {
        let mut body = String::new();
        for delta in deltas {
            let chunk = json!({
                "id": "chatcmpl-test",
                "object": "chat.completion.chunk",
                "created": 0,
                "model": "test-model",
                "choices": [{"index": 0, "delta": {"content": delta}, "finish_reason": null}]
            });
            body.push_str(&format!("data: {chunk}\n\n"));
        }
        body.push_str("data: [DONE]\n\n");
        ResponseTemplate::new(200)
            .insert_header("content-type", "text/event-stream")
            .set_body_string(body)
    }

    /// Answer the next `times` requests with `response`.
    async fn respond(server: &MockServer, response: ResponseTemplate, times: u64) {
        Mock::given(method("POST"))
            .and(path("/v1/chat/completions"))
            .respond_with(response)
            .up_to_n_times(times)
            .mount(server)
            .await;
    }

    fn session(server: &MockServer) -> SessionHandle {
        spawn(ModelClient::new(&ModelConfig {
            base_url: format!("{}/v1", server.uri()),
            api_key: "test".into(),
            model: "test-model".into(),
        }))
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
        respond(&server, reply(&["Hello", ", world"]), 1).await;
        let mut handle = session(&server);

        assert_eq!(
            prompt(&mut handle, "hi").await,
            [delta("Hello"), delta(", world"), Event::TurnEnded]
        );
    }

    #[tokio::test]
    async fn failed_turn_reports_an_error_and_is_left_out_of_history() {
        let server = MockServer::start().await;
        // Not 5xx: async-openai retries those itself.
        respond(&server, ResponseTemplate::new(400), 1).await;
        respond(&server, reply(&["ok"]), 1).await;
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
        respond(&server, reply(&["Hel", "lo"]), 1).await;
        respond(&server, reply(&["Bye"]), 1).await;
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
}
