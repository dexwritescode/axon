//! One conversation: a task that owns the history and runs a turn per prompt.

use std::pin::pin;

use anyhow::Result;
use async_openai::types::chat::ChatCompletionRequestMessage;
use axon_protocol::{Command, Event, SessionHandle, StopReason};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::mcp::Started;
use crate::store::{Saved, Store, Turn};
use crate::turn::{Agent, Approval};

const COMMAND_BUFFER: usize = 16;
pub(crate) const EVENT_BUFFER: usize = 64;

/// Start a session task that saves to `store`, going on from `saved`, and
/// return the frontend's end of it. The task first waits for the context
/// limit and for `mcp`, the MCP servers starting.
pub(crate) fn spawn(
    agent: Agent,
    context_limit: impl Future<Output = Option<u64>> + Send + 'static,
    mcp: impl Future<Output = Started> + Send + 'static,
    store: Store,
    saved: Saved,
) -> SessionHandle {
    let (commands, command_rx) = mpsc::channel(COMMAND_BUFFER);
    let (event_tx, events) = mpsc::channel(EVENT_BUFFER);
    let id = store.id();
    tokio::spawn(run(
        agent,
        context_limit,
        mcp,
        store,
        saved,
        command_rx,
        event_tx,
    ));
    SessionHandle {
        id,
        commands,
        events,
    }
}

/// Runs until the frontend drops its command sender or its event receiver.
async fn run(
    mut agent: Agent,
    context_limit: impl Future<Output = Option<u64>> + Send,
    mcp: impl Future<Output = Started> + Send,
    mut store: Store,
    saved: Saved,
    mut commands: mpsc::Receiver<Command>,
    events: mpsc::Sender<Event>,
) {
    let (tokens, started) = tokio::join!(context_limit, mcp);
    agent.tools.extend(started.schemas);
    agent.mcp = started.mcp;
    if events.send(Event::ContextLimit { tokens }).await.is_err() {
        return;
    }
    if !saved.past.is_empty() {
        let items = saved.past;
        if events.send(Event::History { items }).await.is_err() {
            return;
        }
    }
    for message in started.errors {
        if events.send(Event::Error { message }).await.is_err() {
            return;
        }
    }

    let mut history = saved.history;
    let mut last_call = 0;

    while let Some(command) = commands.recv().await {
        match command {
            Command::Prompt { text } => {
                let mut record = Turn::new(text.clone());
                let before = history.len();
                history.push(ChatCompletionRequestMessage::User(text.into()));
                let result = turn(
                    &agent,
                    &mut history,
                    &mut last_call,
                    &mut commands,
                    &events,
                    &mut record,
                )
                .await;
                let mut ending = Vec::new();
                let stop = match result {
                    Ok(stop) => stop,
                    Err(e) => {
                        history.truncate(before);
                        ending.push(Event::Error {
                            message: format!("{e:#}"),
                        });
                        StopReason::EndTurn
                    }
                };
                record.messages = history[before..].to_vec();
                ending.push(Event::TurnEnded { stop });
                for event in &ending {
                    record.record(event);
                }
                if let Err(e) = store.append(record) {
                    let message = format!("could not save the session: {e:#}");
                    ending.insert(0, Event::Error { message });
                }
                for event in ending {
                    if events.send(event).await.is_err() {
                        return;
                    }
                }
            }
            Command::Cancel | Command::Approval { .. } => {}
        }
    }
}

/// Run a turn while still serving commands: `Cancel` stops it, approvals go
/// to it, and a second `Prompt` is refused. Its events go to the frontend
/// and into `record`. If the frontend goes away, the turn is cancelled.
async fn turn(
    agent: &Agent,
    history: &mut Vec<ChatCompletionRequestMessage>,
    last_call: &mut u64,
    commands: &mut mpsc::Receiver<Command>,
    events: &mpsc::Sender<Event>,
    record: &mut Turn,
) -> Result<StopReason> {
    let cancel = CancellationToken::new();
    let (approve, mut approvals) = mpsc::unbounded_channel();
    let (turn_events, mut from_turn) = mpsc::channel(EVENT_BUFFER);
    let mut turn = pin!(agent.turn(history, &turn_events, &mut approvals, last_call, &cancel));
    let mut frontend_open = true;
    loop {
        tokio::select! {
            biased;
            Some(event) = from_turn.recv() => {
                record.record(&event);
                if events.send(event).await.is_err() {
                    frontend_open = false;
                    cancel.cancel();
                }
            }
            result = &mut turn => {
                while let Ok(event) = from_turn.try_recv() {
                    record.record(&event);
                    let _ = events.send(event).await;
                }
                return result;
            }
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
    use crate::store::Store;
    use crate::test_support::{
        agent, client_error, delta, end, prompt, reply, respond, sent_messages, spawn_saving,
        spawn_started,
    };
    use axon_protocol::{Past, Resume};
    use serde_json::json;
    use std::path::Path;
    use std::time::Duration;
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
        let SessionHandle {
            commands, events, ..
        } = session(&server).await;
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
        assert_eq!(
            sent_messages(&server).await.last().unwrap(),
            &json!([
                {"role": "user", "content": "slow"},
                {"role": "assistant", "content": ""},
                {"role": "user", "content": "after"}
            ])
        );
    }

    #[tokio::test]
    async fn dropping_the_command_sender_mid_turn_cancels_it_and_stops_the_session() {
        let server = MockServer::start().await;
        respond(&server, reply(&["late"]).set_delay(Duration::from_secs(30))).await;
        let SessionHandle {
            commands,
            mut events,
            ..
        } = session(&server).await;

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
        let mut handle = session(&server).await;

        handle.commands.send(Command::Cancel).await.unwrap();

        assert_eq!(
            prompt(&mut handle, "hi").await,
            [delta("ok"), end(StopReason::EndTurn)]
        );
    }

    fn open(root: &Path, cwd: &Path) -> (Store, crate::store::Saved) {
        Store::open(root, cwd, "test-model", &Resume::Latest)
            .unwrap()
            .expect("a saved session")
    }

    #[tokio::test]
    async fn a_resumed_session_shows_what_it_showed_and_sends_the_saved_history() {
        let server = MockServer::start().await;
        respond(&server, reply(&["Hel", "lo"])).await;
        respond(&server, client_error()).await;
        respond(&server, reply(&["Again"])).await;
        let root = tempfile::tempdir().unwrap();
        let cwd = tempfile::tempdir().unwrap();
        let store = Store::new(root.path(), cwd.path(), "test-model");
        let mut handle =
            spawn_saving(agent(&server, cwd.path()), store, Default::default(), ()).await;
        prompt(&mut handle, "hi").await;
        prompt(&mut handle, "fails").await;
        let id = handle.id.clone();
        // The session closes its events once it has ended and let go of the file.
        drop(handle.commands);
        while handle.events.recv().await.is_some() {}

        let (store, saved) = open(root.path(), cwd.path());
        assert_eq!(store.id(), id);
        let SessionHandle {
            commands,
            events: mut all,
            ..
        } = super::spawn(
            agent(&server, cwd.path()),
            async { None },
            async { Started::default() },
            store,
            saved,
        );
        assert_eq!(all.recv().await, Some(Event::ContextLimit { tokens: None }));
        let Some(Event::History { items }) = all.recv().await else {
            panic!("History should follow ContextLimit");
        };
        assert_eq!(
            items[..3],
            [
                Past::Prompt("hi".into()),
                Past::Event(delta("Hello")),
                Past::Event(end(StopReason::EndTurn)),
            ]
        );
        assert_eq!(items[3], Past::Prompt("fails".into()));
        assert!(matches!(&items[4], Past::Event(Event::Error { .. })));
        assert_eq!(items[5], Past::Event(end(StopReason::EndTurn)));

        let mut handle = SessionHandle {
            id,
            commands,
            events: all,
        };
        prompt(&mut handle, "more").await;
        assert_eq!(
            sent_messages(&server).await.last().unwrap(),
            &json!([
                {"role": "user", "content": "hi"},
                {"role": "assistant", "content": "Hello"},
                {"role": "user", "content": "more"}
            ])
        );
    }

    #[tokio::test]
    async fn a_new_session_sends_no_history_event() {
        let server = MockServer::start().await;
        respond(&server, reply(&["ok"])).await;
        let mut handle = session(&server).await;

        assert_eq!(
            prompt(&mut handle, "hi").await,
            [delta("ok"), end(StopReason::EndTurn)]
        );
    }

    #[tokio::test]
    async fn a_turn_that_cannot_be_saved_says_so_and_the_session_goes_on() {
        let server = MockServer::start().await;
        respond(&server, reply(&["one"])).await;
        respond(&server, reply(&["two"])).await;
        let root = tempfile::NamedTempFile::new().unwrap();
        let cwd = tempfile::tempdir().unwrap();
        let store = Store::new(root.path(), cwd.path(), "test-model");
        let mut handle =
            spawn_saving(agent(&server, cwd.path()), store, Default::default(), ()).await;

        let events = prompt(&mut handle, "hi").await;
        assert_eq!(events[0], delta("one"));
        assert!(
            matches!(&events[1], Event::Error { message } if message.starts_with("could not save the session")),
            "{events:?}"
        );
        assert_eq!(events[2], end(StopReason::EndTurn));
        assert_eq!(
            prompt(&mut handle, "again").await.first(),
            Some(&delta("two"))
        );
    }
}
