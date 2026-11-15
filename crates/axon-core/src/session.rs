//! One conversation: a task that owns the history and runs a turn per prompt.

use async_openai::types::chat::ChatCompletionRequestMessage;
use axon_protocol::{Command, Event, SessionHandle, StopReason};
use tokio::sync::mpsc;

use crate::turn::Agent;

const COMMAND_BUFFER: usize = 16;
const EVENT_BUFFER: usize = 64;

/// Start a session task and return the frontend's end of it.
pub(crate) fn spawn(agent: Agent) -> SessionHandle {
    let (commands, command_rx) = mpsc::channel(COMMAND_BUFFER);
    let (event_tx, events) = mpsc::channel(EVENT_BUFFER);
    tokio::spawn(run(agent, command_rx, event_tx));
    SessionHandle { commands, events }
}

/// Runs until the frontend drops its command sender or its event receiver.
async fn run(agent: Agent, mut commands: mpsc::Receiver<Command>, events: mpsc::Sender<Event>) {
    let mut history: Vec<ChatCompletionRequestMessage> = Vec::new();

    while let Some(command) = commands.recv().await {
        match command {
            Command::Prompt { text } => {
                let before = history.len();
                history.push(ChatCompletionRequestMessage::User(text.into()));
                let stop = match agent.turn(&mut history, &events).await {
                    Ok(stop) => stop,
                    Err(e) => {
                        history.truncate(before);
                        let message = format!("{e:#}");
                        if events.send(Event::Error { message }).await.is_err() {
                            return;
                        }
                        StopReason::EndTurn
                    }
                };
                if events.send(Event::TurnEnded { stop }).await.is_err() {
                    return;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{agent, client_error, prompt, reply, respond, sent_messages};
    use serde_json::json;
    use std::path::Path;
    use std::time::Duration;
    use wiremock::MockServer;

    fn session(server: &MockServer) -> SessionHandle {
        spawn(agent(server, Path::new(".")))
    }

    const END: Event = Event::TurnEnded {
        stop: StopReason::EndTurn,
    };

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
            [delta("Hello"), delta(", world"), END]
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
            matches!(events[..], [Event::Error { .. }, END]),
            "{events:?}"
        );

        assert_eq!(prompt(&mut handle, "second").await, [delta("ok"), END]);
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
