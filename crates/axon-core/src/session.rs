//! One conversation: a task that owns the history and runs a turn per prompt.

use std::pin::pin;

use anyhow::Result;
use async_openai::types::chat::ChatCompletionRequestMessage;
use axon_protocol::{Command, Event, SessionHandle, StopReason};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::turn::{Agent, Approval};

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
    let mut last_approval = 0;

    while let Some(command) = commands.recv().await {
        match command {
            Command::Prompt { text } => {
                let before = history.len();
                history.push(ChatCompletionRequestMessage::User(text.into()));
                let result = turn(
                    &agent,
                    &mut history,
                    &mut last_approval,
                    &mut commands,
                    &events,
                )
                .await;
                let stop = match result {
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
            Command::Cancel | Command::Approval { .. } => {}
        }
    }
}

/// Run a turn while still serving commands: `Cancel` stops it, approvals go
/// to it, and a second `Prompt` is refused. If the frontend goes away, the
/// turn is cancelled.
async fn turn(
    agent: &Agent,
    history: &mut Vec<ChatCompletionRequestMessage>,
    last_approval: &mut u64,
    commands: &mut mpsc::Receiver<Command>,
    events: &mpsc::Sender<Event>,
) -> Result<StopReason> {
    let cancel = CancellationToken::new();
    let (approve, mut approvals) = mpsc::unbounded_channel();
    let mut turn = pin!(agent.turn(history, events, &mut approvals, last_approval, &cancel));
    let mut frontend_open = true;
    loop {
        tokio::select! {
            result = &mut turn => return result,
            command = commands.recv(), if frontend_open => match command {
                Some(Command::Cancel) => cancel.cancel(),
                Some(Command::Approval { id, approve: yes }) => {
                    let _ = approve.send(Approval { id, approve: yes });
                }
                Some(Command::Prompt { .. }) => {
                    let message = "a turn is already running".to_string();
                    let _ = events.send(Event::Error { message }).await;
                }
                None => {
                    frontend_open = false;
                    cancel.cancel();
                }
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{agent, delta, end, prompt, reply, respond, sent_messages};
    use serde_json::json;
    use std::path::Path;
    use std::time::Duration;
    use wiremock::matchers::body_string_contains;
    use wiremock::{Mock, MockServer};

    fn session(server: &MockServer) -> SessionHandle {
        spawn(agent(server, Path::new(".")))
    }

    #[tokio::test]
    async fn prompt_streams_deltas_then_ends_the_turn() {
        let server = MockServer::start().await;
        respond(&server, reply(&["Hello", ", world"])).await;
        let mut handle = session(&server);

        assert_eq!(
            prompt(&mut handle, "hi").await,
            [delta("Hello"), delta(", world"), end(StopReason::EndTurn)]
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

    #[tokio::test]
    async fn cancel_stops_a_turn_waiting_on_the_model_and_refuses_a_second_prompt() {
        let server = MockServer::start().await;
        respond(&server, reply(&["late"]).set_delay(Duration::from_secs(30))).await;
        // A cancelled request isn't counted as answered, so match the next one by its prompt.
        Mock::given(body_string_contains("after"))
            .respond_with(reply(&["next"]))
            .with_priority(1)
            .mount(&server)
            .await;
        let mut handle = session(&server);
        let send = |command| handle.commands.send(command);

        send(Command::Prompt {
            text: "slow".into(),
        })
        .await
        .unwrap();
        send(Command::Prompt {
            text: "again".into(),
        })
        .await
        .unwrap();
        send(Command::Cancel).await.unwrap();

        let events = tokio::time::timeout(Duration::from_secs(5), async {
            [handle.events.recv().await, handle.events.recv().await]
        })
        .await
        .expect("turn should stop without waiting for the model");
        assert_eq!(
            events,
            [
                Some(Event::Error {
                    message: "a turn is already running".into()
                }),
                Some(end(StopReason::Cancelled))
            ]
        );

        assert_eq!(
            prompt(&mut handle, "after").await,
            [delta("next"), end(StopReason::EndTurn)]
        );
    }

    #[tokio::test]
    async fn dropping_the_command_sender_mid_turn_cancels_it_and_stops_the_session() {
        let server = MockServer::start().await;
        respond(&server, reply(&["late"]).set_delay(Duration::from_secs(30))).await;
        let SessionHandle {
            commands,
            mut events,
        } = session(&server);

        commands
            .send(Command::Prompt { text: "hi".into() })
            .await
            .unwrap();
        drop(commands);

        let events = tokio::time::timeout(Duration::from_secs(5), async {
            [events.recv().await, events.recv().await]
        })
        .await
        .expect("turn should stop without waiting for the model");
        assert_eq!(events, [Some(end(StopReason::Cancelled)), None]);
    }

    #[tokio::test]
    async fn cancel_while_idle_does_nothing() {
        let server = MockServer::start().await;
        respond(&server, reply(&["ok"])).await;
        let mut handle = session(&server);

        handle.commands.send(Command::Cancel).await.unwrap();

        assert_eq!(
            prompt(&mut handle, "hi").await,
            [delta("ok"), end(StopReason::EndTurn)]
        );
    }
}
