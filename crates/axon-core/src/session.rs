//! One conversation: a task that owns the history and runs a turn per prompt.

use std::collections::VecDeque;
use std::pin::pin;

use anyhow::Result;
use async_openai::types::chat::ChatCompletionRequestMessage;
use axon_protocol::{Command, Event, SessionHandle, StopReason};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::turn::{Agent, Approval};

const COMMAND_BUFFER: usize = 16;
pub(crate) const EVENT_BUFFER: usize = 64;

/// Start a session task and return the frontend's end of it.
pub(crate) fn spawn(
    agent: Agent,
    context_limit: impl Future<Output = Option<u64>> + Send + 'static,
) -> SessionHandle {
    let (commands, command_rx) = mpsc::channel(COMMAND_BUFFER);
    let (event_tx, events) = mpsc::channel(EVENT_BUFFER);
    tokio::spawn(run(agent, context_limit, command_rx, event_tx));
    SessionHandle { commands, events }
}

/// Runs until the frontend drops its command sender or its event receiver.
async fn run(
    agent: Agent,
    context_limit: impl Future<Output = Option<u64>> + Send,
    mut commands: mpsc::Receiver<Command>,
    events: mpsc::Sender<Event>,
) {
    let tokens = context_limit.await;
    if events.send(Event::ContextLimit { tokens }).await.is_err() {
        return;
    }

    let mut history: Vec<ChatCompletionRequestMessage> = Vec::new();
    let mut last_call = 0;
    let mut leftover = VecDeque::new();

    loop {
        let text = match leftover.pop_front() {
            Some(text) => text,
            None => match commands.recv().await {
                Some(Command::Prompt { text }) => text,
                Some(Command::Cancel | Command::Approval { .. }) => continue,
                None => return,
            },
        };
        let before = history.len();
        history.push(ChatCompletionRequestMessage::User(text.clone().into()));
        if events.send(Event::UserMessage { text }).await.is_err() {
            return;
        }
        let (steer, mut steers) = mpsc::unbounded_channel();
        for text in leftover.drain(..) {
            let _ = steer.send(text);
        }
        let result = turn(
            &agent,
            &mut history,
            &mut last_call,
            &mut commands,
            (&steer, &mut steers),
            &events,
        )
        .await;
        while let Ok(text) = steers.try_recv() {
            leftover.push_back(text);
        }
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
        if events.send(Event::TurnEnded { stop }).await.is_err() || commands.is_closed() {
            return;
        }
    }
}

/// Run a turn while still serving commands: `Cancel` stops it, approvals go
/// to it, and a `Prompt` joins it before its next model request. If the
/// frontend goes away, the turn is cancelled.
async fn turn(
    agent: &Agent,
    history: &mut Vec<ChatCompletionRequestMessage>,
    last_call: &mut u64,
    commands: &mut mpsc::Receiver<Command>,
    (steer, steers): (
        &mpsc::UnboundedSender<String>,
        &mut mpsc::UnboundedReceiver<String>,
    ),
    events: &mpsc::Sender<Event>,
) -> Result<StopReason> {
    let cancel = CancellationToken::new();
    let (approve, mut approvals) = mpsc::unbounded_channel();
    let mut turn = pin!(agent.turn(history, events, &mut approvals, steers, last_call, &cancel));
    let mut frontend_open = true;
    loop {
        tokio::select! {
            biased;
            result = &mut turn => return result,
            command = commands.recv(), if frontend_open => match command {
                Some(Command::Cancel) => cancel.cancel(),
                Some(Command::Approval { id, approve: yes }) => {
                    let _ = approve.send(Approval { id, approve: yes });
                }
                Some(Command::Prompt { text }) => {
                    let _ = steer.send(text);
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
    use crate::test_support::{
        agent, delta, end, prompt, reply, respond, sent_messages, spawn_started, tool_calls,
    };
    use serde_json::json;
    use std::path::Path;
    use std::time::Duration;
    use tempfile::TempDir;
    use wiremock::matchers::body_string_contains;
    use wiremock::{Mock, MockServer};

    async fn session(server: &MockServer) -> SessionHandle {
        spawn_started(agent(server, Path::new("."))).await
    }

    #[tokio::test]
    async fn prompt_streams_deltas_then_ends_the_turn() {
        let server = MockServer::start().await;
        respond(&server, reply(&["Hello", ", world"])).await;
        let mut handle = session(&server).await;

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
        let mut handle = session(&server).await;

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
        let SessionHandle { commands, events } = session(&server).await;
        drop(events);

        commands
            .send(Command::Prompt { text: "hi".into() })
            .await
            .unwrap();

        tokio::time::timeout(Duration::from_secs(5), commands.closed())
            .await
            .expect("session should stop");
    }

    /// Collect events up to and including the `turns`th `TurnEnded`, leaving
    /// out tool call events. Each approval request is met with a "steer"
    /// prompt, then answered with `approve`.
    async fn turns(handle: &mut SessionHandle, turns: usize, approve: bool) -> Vec<Event> {
        let mut events = Vec::new();
        let mut ended = 0;
        while ended < turns {
            let event = tokio::time::timeout(Duration::from_secs(5), handle.events.recv())
                .await
                .expect("turn should end")
                .unwrap();
            if let Event::ApprovalRequested { id, .. } = event {
                let steer = Command::Prompt {
                    text: "steer".into(),
                };
                handle.commands.send(steer).await.unwrap();
                handle
                    .commands
                    .send(Command::Approval { id, approve })
                    .await
                    .unwrap();
            }
            ended += usize::from(matches!(event, Event::TurnEnded { .. }));
            if matches!(
                event,
                Event::UserMessage { .. } | Event::TextDelta { .. } | Event::TurnEnded { .. }
            ) {
                events.push(event);
            }
        }
        events
    }

    fn user(text: &str) -> Event {
        Event::UserMessage { text: text.into() }
    }

    async fn write(handle: &SessionHandle, path: &str) {
        let text = format!("write {path}");
        handle
            .commands
            .send(Command::Prompt { text })
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn a_prompt_sent_during_a_turn_joins_it_before_the_next_request() {
        let server = MockServer::start().await;
        let dir = TempDir::new().unwrap();
        let args = r#"{"path":"a.txt","content":"x"}"#;
        respond(&server, tool_calls(&[("c1", "write_file", args)])).await;
        respond(&server, reply(&["done"])).await;
        let mut handle = spawn_started(agent(&server, dir.path())).await;

        write(&handle, "a.txt").await;

        assert_eq!(
            turns(&mut handle, 1, true).await,
            [
                user("write a.txt"),
                user("steer"),
                delta("done"),
                end(StopReason::EndTurn)
            ]
        );
        let second = &sent_messages(&server).await[1];
        assert_eq!(second[2]["role"], "tool");
        assert_eq!(second[3], json!({"role": "user", "content": "steer"}));
    }

    #[tokio::test]
    async fn a_prompt_the_turn_ends_without_starts_the_next_turn() {
        let server = MockServer::start().await;
        let dir = TempDir::new().unwrap();
        let args = r#"{"path":"b.txt","content":"x"}"#;
        respond(&server, tool_calls(&[("c1", "write_file", args)])).await;
        respond(&server, reply(&["ok"])).await;
        let mut handle = spawn_started(agent(&server, dir.path())).await;

        write(&handle, "b.txt").await;

        assert_eq!(
            turns(&mut handle, 2, false).await,
            [
                user("write b.txt"),
                end(StopReason::EndTurn),
                user("steer"),
                delta("ok"),
                end(StopReason::EndTurn)
            ]
        );
    }

    #[tokio::test]
    async fn a_prompt_sent_before_a_cancel_starts_the_next_turn() {
        let server = MockServer::start().await;
        respond(&server, reply(&["late"]).set_delay(Duration::from_secs(30))).await;
        // A cancelled request isn't counted as answered, so match the next one by its prompt.
        Mock::given(body_string_contains("again"))
            .respond_with(reply(&["next"]))
            .with_priority(1)
            .mount(&server)
            .await;
        let mut handle = session(&server).await;
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

        assert_eq!(
            turns(&mut handle, 2, true).await,
            [
                user("slow"),
                end(StopReason::Cancelled),
                user("again"),
                delta("next"),
                end(StopReason::EndTurn)
            ]
        );
    }

    #[tokio::test]
    async fn dropping_the_command_sender_mid_turn_cancels_it_and_skips_waiting_prompts() {
        let server = MockServer::start().await;
        respond(&server, reply(&["late"]).set_delay(Duration::from_secs(30))).await;
        let SessionHandle {
            commands,
            mut events,
        } = session(&server).await;

        commands
            .send(Command::Prompt { text: "hi".into() })
            .await
            .unwrap();
        commands
            .send(Command::Prompt {
                text: "steer".into(),
            })
            .await
            .unwrap();
        drop(commands);

        let events = tokio::time::timeout(Duration::from_secs(5), async {
            [
                events.recv().await,
                events.recv().await,
                events.recv().await,
            ]
        })
        .await
        .expect("turn should stop without waiting for the model");
        assert_eq!(
            events,
            [Some(user("hi")), Some(end(StopReason::Cancelled)), None]
        );
    }

    #[tokio::test]
    async fn cancel_while_idle_does_nothing() {
        let server = MockServer::start().await;
        respond(&server, reply(&["ok"])).await;
        let mut handle = session(&server).await;

        handle.commands.send(Command::Cancel).await.unwrap();

        assert_eq!(
            prompt(&mut handle, "hi").await,
            [delta("ok"), end(StopReason::EndTurn)]
        );
    }
}
