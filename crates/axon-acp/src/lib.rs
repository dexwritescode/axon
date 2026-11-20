//! ACP (Agent Client Protocol) frontend: serves core sessions to an ACP
//! client, such as Zed, over stdio.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use agent_client_protocol::schema::ProtocolVersion;
use agent_client_protocol::schema::v1::{
    AgentCapabilities, CancelNotification, ContentBlock, ContentChunk, Diff, InitializeRequest,
    InitializeResponse, NewSessionRequest, NewSessionResponse, PermissionOption,
    PermissionOptionKind, PromptRequest, PromptResponse, RequestPermissionOutcome,
    RequestPermissionRequest, SessionNotification, SessionUpdate, StopReason as AcpStop, ToolCall,
    ToolCallContent, ToolCallStatus, ToolCallUpdate, ToolCallUpdateFields, ToolKind,
};
use agent_client_protocol::{Agent, Client, ConnectTo, ConnectionTo, Error, Stdio};
use axon_protocol::{Change, Command, Event, Exit, Resume, SessionFactory, StopReason, ToolResult};
use tokio::sync::mpsc;

/// The permission option that approves a change.
const ALLOW: &str = "allow";
/// The permission option that rejects a change.
const REJECT: &str = "reject";

/// Why serving ACP stopped.
#[derive(Debug, thiserror::Error)]
#[error("ACP: {0}")]
pub struct AcpError(#[from] Error);

/// Serve ACP on stdin/stdout until the client disconnects; each
/// `session/new` starts a session from `factory`.
pub async fn serve(factory: Arc<dyn SessionFactory>) -> Result<(), AcpError> {
    Ok(serve_on(factory, Stdio::new()).await?)
}

/// What the connection keeps per session.
struct Session {
    /// The client's working directory; tool paths are relative to it.
    cwd: PathBuf,
    commands: mpsc::Sender<Command>,
    /// Taken by the running prompt; None while a turn runs.
    events: Option<mpsc::Receiver<Event>>,
}

type Sessions = Arc<Mutex<HashMap<String, Session>>>;

/// Serve ACP on `transport` until the client disconnects.
async fn serve_on(
    factory: Arc<dyn SessionFactory>,
    transport: impl ConnectTo<Agent>,
) -> Result<(), Error> {
    let sessions: Sessions = Arc::default();
    let (for_new, for_prompt, for_cancel) = (sessions.clone(), sessions.clone(), sessions);
    Agent
        .builder()
        .name("Axon")
        .on_receive_request(
            async |initialize: InitializeRequest, responder, _connection| {
                // The client's version if we speak it, else our latest.
                let version = initialize.protocol_version.min(ProtocolVersion::LATEST);
                responder.respond(
                    InitializeResponse::new(version).agent_capabilities(AgentCapabilities::new()),
                )
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            async move |request: NewSessionRequest, responder, _connection| {
                if !request.mcp_servers.is_empty() {
                    eprintln!(
                        "axon: MCP servers aren't supported yet; ignoring {}",
                        request.mcp_servers.len()
                    );
                }
                match factory.new_session(request.cwd.clone(), Resume::New) {
                    Ok(handle) => {
                        let id = handle.id.clone();
                        let session = Session {
                            cwd: request.cwd,
                            commands: handle.commands,
                            events: Some(handle.events),
                        };
                        lock(&for_new).insert(id.clone(), session);
                        responder.respond(NewSessionResponse::new(id))
                    }
                    Err(e) => responder.respond_with_error(internal(e)),
                }
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            async move |request: PromptRequest, responder, connection| {
                let id = request.session_id.to_string();
                let text = match prompt_text(request.prompt) {
                    Ok(text) => text,
                    Err(e) => return responder.respond_with_error(e),
                };
                let taken = match lock(&for_prompt).get_mut(&id) {
                    None => Err(Error::session_not_found(id.clone())),
                    Some(session) => match session.events.take() {
                        None => Err(invalid("a turn is already running")),
                        Some(events) => Ok((session.cwd.clone(), session.commands.clone(), events)),
                    },
                };
                let (cwd, commands, mut events) = match taken {
                    Ok(taken) => taken,
                    Err(e) => return responder.respond_with_error(e),
                };
                // The turn runs outside the dispatch loop, so the connection
                // keeps serving `session/cancel` and permission answers.
                let sessions = for_prompt.clone();
                let task_connection = connection.clone();
                connection.spawn(async move {
                    let prompt = Prompt {
                        connection: &task_connection,
                        id: &id,
                        cwd: &cwd,
                        commands: &commands,
                    };
                    let stop = prompt.run(text, &mut events).await;
                    // After an error the turn may still be running, so its
                    // events can't go to the next prompt: drop the session,
                    // which cancels the turn.
                    let mut sessions = lock(&sessions);
                    match (&stop, sessions.get_mut(&id)) {
                        (Ok(_), Some(session)) => session.events = Some(events),
                        _ => drop(sessions.remove(&id)),
                    }
                    drop(sessions);
                    match stop {
                        Ok(stop) => responder.respond(PromptResponse::new(stop)),
                        Err(e) => responder.respond_with_error(e),
                    }
                })
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_notification(
            async move |cancel: CancelNotification, _connection| {
                let commands = lock(&for_cancel)
                    .get(&cancel.session_id.to_string())
                    .map(|session| session.commands.clone());
                if let Some(commands) = commands {
                    // Not `send().await`: that could hold the dispatch loop.
                    let _ = commands.try_send(Command::Cancel);
                }
                Ok(())
            },
            agent_client_protocol::on_receive_notification!(),
        )
        .connect_to(transport)
        .await
}

/// One `session/prompt` in flight.
struct Prompt<'a> {
    connection: &'a ConnectionTo<Client>,
    id: &'a str,
    cwd: &'a Path,
    commands: &'a mpsc::Sender<Command>,
}

impl Prompt<'_> {
    /// Send `text` as a prompt and relay the turn's events until it ends.
    async fn run(
        &self,
        text: String,
        events: &mut mpsc::Receiver<Event>,
    ) -> Result<AcpStop, Error> {
        let ended = || internal("the session ended");
        self.commands
            .send(Command::Prompt { text })
            .await
            .map_err(|_| ended())?;
        loop {
            match events.recv().await.ok_or_else(ended)? {
                Event::TurnEnded { stop } => return Ok(stop_reason(stop)),
                Event::TextDelta { text } => self.update(message(text))?,
                Event::ReasoningDelta { text } => self.update(SessionUpdate::AgentThoughtChunk(
                    ContentChunk::new(text.into()),
                ))?,
                Event::Error { message: text } => {
                    self.update(message(format!("Error: {text}\n")))?
                }
                Event::ToolCallStarted { id, name, args } => {
                    let call = ToolCall::new(id.to_string(), name.clone())
                        .kind(kind(&name))
                        .status(ToolCallStatus::InProgress)
                        .raw_input(serde_json::from_str(&args).ok());
                    self.update(SessionUpdate::ToolCall(call))?
                }
                Event::ToolCallFinished { id, result } => {
                    let (status, text) = finished(result);
                    let fields = ToolCallUpdateFields::new()
                        .status(status)
                        .content(vec![ToolCallContent::from(text)]);
                    self.update(SessionUpdate::ToolCallUpdate(ToolCallUpdate::new(
                        id.to_string(),
                        fields,
                    )))?
                }
                Event::ApprovalRequested { id, change } => {
                    let approve = self.ask(id, change).await;
                    self.commands
                        .send(Command::Approval { id, approve })
                        .await
                        .map_err(|_| ended())?;
                }
                Event::ContextLimit { .. }
                | Event::History { .. }
                | Event::ToolCallStreaming { .. }
                | Event::Paused { .. } => {}
            }
        }
    }

    fn update(&self, update: SessionUpdate) -> Result<(), Error> {
        self.connection
            .send_notification(SessionNotification::new(self.id.to_string(), update))
    }

    /// Ask the client to approve `change` for tool call `id`. Anything but
    /// the allow option, including a cancelled request or an error, rejects it.
    /// On `session/cancel` the client must answer a pending request with
    /// cancelled (ACP requires it); until it does, this waits.
    async fn ask(&self, id: u64, change: Change) -> bool {
        let fields = match change {
            Change::WriteFile {
                path,
                old_text,
                new_text,
            } => ToolCallUpdateFields::new()
                .title(format!("Write {path}"))
                .content(vec![ToolCallContent::Diff(
                    Diff::new(self.cwd.join(&path), new_text).old_text(old_text),
                )]),
            Change::RunCommand { command } => {
                ToolCallUpdateFields::new().title(format!("Run {command}"))
            }
        };
        let options = vec![
            PermissionOption::new(ALLOW, "Allow", PermissionOptionKind::AllowOnce),
            PermissionOption::new(REJECT, "Reject", PermissionOptionKind::RejectOnce),
        ];
        let request = RequestPermissionRequest::new(
            self.id.to_string(),
            ToolCallUpdate::new(id.to_string(), fields),
            options,
        );
        match self.connection.send_request(request).block_task().await {
            Ok(response) => matches!(
                response.outcome,
                RequestPermissionOutcome::Selected(selected) if selected.option_id.to_string() == ALLOW
            ),
            Err(_) => false,
        }
    }
}

fn lock(sessions: &Sessions) -> std::sync::MutexGuard<'_, HashMap<String, Session>> {
    sessions.lock().expect("sessions lock")
}

fn invalid(message: impl ToString) -> Error {
    Error::invalid_params().data(message.to_string())
}

fn internal(message: impl ToString) -> Error {
    Error::internal_error().data(message.to_string())
}

fn message(text: String) -> SessionUpdate {
    SessionUpdate::AgentMessageChunk(ContentChunk::new(text.into()))
}

/// The prompt's text blocks joined; any other kind of block is an error.
fn prompt_text(prompt: Vec<ContentBlock>) -> Result<String, Error> {
    let mut texts = Vec::new();
    for block in prompt {
        match block {
            ContentBlock::Text(text) => texts.push(text.text),
            _ => return Err(invalid("only text prompts are supported")),
        }
    }
    Ok(texts.join("\n"))
}

/// What kind of tool the client shows, from its name.
fn kind(name: &str) -> ToolKind {
    match name {
        "read_file" => ToolKind::Read,
        "list_dir" | "grep" | "find" => ToolKind::Search,
        "write_file" | "edit_file" => ToolKind::Edit,
        "run_command" => ToolKind::Execute,
        _ => ToolKind::Other,
    }
}

/// The status and text the client shows for a finished tool call.
fn finished(result: ToolResult) -> (ToolCallStatus, String) {
    match result {
        ToolResult::Output(output) => (ToolCallStatus::Completed, output),
        ToolResult::Failed(error) => (ToolCallStatus::Failed, error),
        ToolResult::Rejected => (ToolCallStatus::Failed, "Rejected".into()),
        ToolResult::Command {
            head,
            cut,
            tail,
            exit,
        } => {
            let mut text = head;
            if cut > 0 {
                text.push_str(&format!("\n[{cut} bytes cut]\n"));
                text.push_str(&tail);
            }
            let (status, ended) = match exit {
                Exit::Code(0) => (ToolCallStatus::Completed, "exit code 0".to_string()),
                Exit::Code(code) => (ToolCallStatus::Failed, format!("exit code {code}")),
                Exit::Killed => (ToolCallStatus::Failed, "killed".into()),
                Exit::TimedOut { secs } => {
                    (ToolCallStatus::Failed, format!("timed out after {secs} s"))
                }
                Exit::Cancelled => (ToolCallStatus::Failed, "cancelled".into()),
            };
            if !text.is_empty() && !text.ends_with('\n') {
                text.push('\n');
            }
            text.push_str(&format!("[{ended}]"));
            (status, text)
        }
    }
}

fn stop_reason(stop: StopReason) -> AcpStop {
    match stop {
        StopReason::EndTurn => AcpStop::EndTurn,
        StopReason::Cancelled => AcpStop::Cancelled,
        StopReason::MaxTurnRequests => AcpStop::MaxTurnRequests,
        StopReason::MaxTokens => AcpStop::MaxTokens,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use agent_client_protocol::Channel;
    use agent_client_protocol::schema::ProtocolVersion;
    use agent_client_protocol::schema::v1::{
        RequestPermissionResponse, SelectedPermissionOutcome, TextContent,
    };
    use axon_protocol::{SessionError, SessionHandle};
    use std::path::PathBuf;
    use std::time::Duration;
    use tokio::sync::oneshot;

    /// The core's end of a stub session, which the test plays.
    struct Core {
        cwd: PathBuf,
        commands: mpsc::Receiver<Command>,
        events: mpsc::Sender<Event>,
    }

    impl Core {
        async fn next(&mut self) -> Command {
            tokio::time::timeout(Duration::from_secs(5), self.commands.recv())
                .await
                .expect("no command")
                .expect("commands closed")
        }

        async fn send(&self, event: Event) {
            self.events.send(event).await.unwrap();
        }
    }

    /// Hands each new session's core end to the test; fails to start
    /// sessions in a cwd named "/broken".
    #[derive(Default)]
    struct Stub {
        cores: Mutex<Vec<Core>>,
    }

    impl SessionFactory for Stub {
        fn new_session(&self, cwd: PathBuf, resume: Resume) -> Result<SessionHandle, SessionError> {
            assert_eq!(resume, Resume::New);
            if cwd == Path::new("/broken") {
                return Err(SessionError::Start("no such folder".into()));
            }
            let (commands, command_rx) = mpsc::channel(16);
            let (event_tx, events) = mpsc::channel(64);
            let mut cores = self.cores.lock().unwrap();
            let id = format!("s{}", cores.len() + 1);
            cores.push(Core {
                cwd,
                commands: command_rx,
                events: event_tx,
            });
            Ok(SessionHandle {
                id,
                commands,
                events,
            })
        }
    }

    /// A client connected in process to `serve_on`, with one session open.
    struct Test {
        connection: ConnectionTo<Agent>,
        updates: mpsc::UnboundedReceiver<SessionUpdate>,
        asked: mpsc::UnboundedReceiver<RequestPermissionRequest>,
        session: String,
        core: Core,
    }

    /// How the test client answers permission requests.
    #[derive(Clone, Copy)]
    enum Answer {
        Allow,
        Reject,
        Cancel,
        /// Answer with a JSON-RPC error.
        Fail,
    }

    async fn start(answer: Answer) -> Test {
        let stub = Arc::new(Stub::default());
        let (agent_end, client_end) = Channel::duplex();
        tokio::spawn(serve_on(stub.clone(), agent_end));
        let (update_tx, updates) = mpsc::unbounded_channel();
        let (ask_tx, asked) = mpsc::unbounded_channel();
        let (connected, connection) = oneshot::channel();
        tokio::spawn(
            Client
                .builder()
                .on_receive_notification(
                    async move |notification: SessionNotification, _connection| {
                        let _ = update_tx.send(notification.update);
                        Ok(())
                    },
                    agent_client_protocol::on_receive_notification!(),
                )
                .on_receive_request(
                    async move |request: RequestPermissionRequest, responder, _connection| {
                        let _ = ask_tx.send(request);
                        if let Answer::Fail = answer {
                            return responder.respond_with_error(Error::internal_error());
                        }
                        let outcome = match answer {
                            Answer::Allow => RequestPermissionOutcome::Selected(
                                SelectedPermissionOutcome::new(ALLOW),
                            ),
                            Answer::Reject => RequestPermissionOutcome::Selected(
                                SelectedPermissionOutcome::new(REJECT),
                            ),
                            Answer::Cancel | Answer::Fail => RequestPermissionOutcome::Cancelled,
                        };
                        responder.respond(RequestPermissionResponse::new(outcome))
                    },
                    agent_client_protocol::on_receive_request!(),
                )
                .connect_with(client_end, async move |connection| {
                    let _ = connected.send(connection);
                    std::future::pending::<Result<(), Error>>().await
                }),
        );
        let connection = connection.await.unwrap();
        connection
            .send_request(InitializeRequest::new(ProtocolVersion::V1))
            .block_task()
            .await
            .unwrap();
        let session = connection
            .send_request(NewSessionRequest::new("/work"))
            .block_task()
            .await
            .unwrap()
            .session_id
            .to_string();
        let core = stub.cores.lock().unwrap().pop().unwrap();
        Test {
            connection,
            updates,
            asked,
            session,
            core,
        }
    }

    impl Test {
        /// Send `session/prompt` with these text blocks; the response comes on the receiver.
        fn prompt(&self, texts: &[&str]) -> oneshot::Receiver<Result<PromptResponse, Error>> {
            let blocks = texts
                .iter()
                .map(|text| ContentBlock::Text(TextContent::new(*text)))
                .collect();
            self.send_prompt(blocks)
        }

        fn send_prompt(
            &self,
            blocks: Vec<ContentBlock>,
        ) -> oneshot::Receiver<Result<PromptResponse, Error>> {
            let (tx, rx) = oneshot::channel();
            let request = PromptRequest::new(self.session.clone(), blocks);
            let connection = self.connection.clone();
            tokio::spawn(async move {
                let response = connection.send_request(request).block_task().await;
                let _ = tx.send(response);
            });
            rx
        }

        /// The updates received so far.
        fn updates(&mut self) -> Vec<SessionUpdate> {
            let mut updates = Vec::new();
            while let Ok(update) = self.updates.try_recv() {
                updates.push(update);
            }
            updates
        }
    }

    async fn response(
        rx: oneshot::Receiver<Result<PromptResponse, Error>>,
    ) -> Result<PromptResponse, Error> {
        tokio::time::timeout(Duration::from_secs(5), rx)
            .await
            .expect("no prompt response")
            .unwrap()
    }

    fn text_of(update: &SessionUpdate) -> Option<(&'static str, String)> {
        let (kind, chunk) = match update {
            SessionUpdate::AgentMessageChunk(chunk) => ("message", chunk),
            SessionUpdate::AgentThoughtChunk(chunk) => ("thought", chunk),
            _ => return None,
        };
        match &chunk.content {
            ContentBlock::Text(text) => Some((kind, text.text.clone())),
            _ => None,
        }
    }

    #[tokio::test]
    async fn a_new_session_starts_in_the_clients_cwd() {
        let test = start(Answer::Allow).await;

        assert_eq!(test.core.cwd, PathBuf::from("/work"));
        assert_eq!(test.session, "s1");
    }

    #[tokio::test]
    async fn a_prompt_streams_the_reply_and_ends_with_the_stop_reason() {
        let mut test = start(Answer::Allow).await;

        let pending = test.prompt(&["hello", "world"]);
        assert!(matches!(
            test.core.next().await,
            Command::Prompt { text } if text == "hello\nworld"
        ));
        for event in [
            Event::ReasoningDelta { text: "hmm".into() },
            Event::TextDelta { text: "Hi ".into() },
            Event::TextDelta {
                text: "there".into(),
            },
            Event::Error {
                message: "slow server".into(),
            },
            Event::TurnEnded {
                stop: StopReason::EndTurn,
            },
        ] {
            test.core.send(event).await;
        }

        let response = response(pending).await.unwrap();
        assert_eq!(response.stop_reason, AcpStop::EndTurn);
        let texts: Vec<_> = test.updates().iter().filter_map(text_of).collect();
        assert_eq!(
            texts,
            [
                ("thought", "hmm".into()),
                ("message", "Hi ".into()),
                ("message", "there".into()),
                ("message", "Error: slow server\n".into()),
            ]
        );
    }

    #[tokio::test]
    async fn initialize_offers_the_clients_version_only_if_we_speak_it() {
        let test = start(Answer::Allow).await;
        for (asked, offered) in [(1, 1), (2, 1)] {
            let response = test
                .connection
                .send_request(InitializeRequest::new(ProtocolVersion::from(asked)))
                .block_task()
                .await
                .unwrap();
            assert_eq!(response.protocol_version, ProtocolVersion::from(offered));
        }
    }

    #[tokio::test]
    async fn a_session_that_fails_to_start_is_an_error() {
        let test = start(Answer::Allow).await;

        let error = test
            .connection
            .send_request(NewSessionRequest::new("/broken"))
            .block_task()
            .await
            .unwrap_err();

        let data = error.data.unwrap().to_string();
        assert!(data.contains("no such folder"), "{data}");
    }

    #[test]
    fn tool_kinds_follow_the_tool_names() {
        let kinds: Vec<_> = [
            "read_file",
            "list_dir",
            "grep",
            "find",
            "write_file",
            "edit_file",
            "run_command",
            "get_time",
        ]
        .into_iter()
        .map(kind)
        .collect();
        assert_eq!(
            kinds,
            [
                ToolKind::Read,
                ToolKind::Search,
                ToolKind::Search,
                ToolKind::Search,
                ToolKind::Edit,
                ToolKind::Edit,
                ToolKind::Execute,
                ToolKind::Other,
            ]
        );
    }

    #[tokio::test]
    async fn tool_calls_become_tool_call_updates() {
        let mut test = start(Answer::Allow).await;
        let pending = test.prompt(&["look"]);
        test.core.next().await;
        test.core
            .send(Event::ToolCallStarted {
                id: 1,
                name: "grep".into(),
                args: r#"{"pattern":"x"}"#.into(),
            })
            .await;
        test.core
            .send(Event::ToolCallFinished {
                id: 1,
                result: ToolResult::Output("a.rs:1: x".into()),
            })
            .await;
        test.core
            .send(Event::TurnEnded {
                stop: StopReason::EndTurn,
            })
            .await;
        response(pending).await.unwrap();

        let updates = test.updates();
        let SessionUpdate::ToolCall(call) = &updates[0] else {
            panic!("{updates:?}");
        };
        assert_eq!(call.tool_call_id.to_string(), "1");
        assert_eq!(call.title, "grep");
        assert_eq!(call.kind, ToolKind::Search);
        assert_eq!(call.status, ToolCallStatus::InProgress);
        assert_eq!(call.raw_input, Some(serde_json::json!({"pattern": "x"})));
        let SessionUpdate::ToolCallUpdate(update) = &updates[1] else {
            panic!("{updates:?}");
        };
        assert_eq!(update.tool_call_id.to_string(), "1");
        assert_eq!(update.fields.status, Some(ToolCallStatus::Completed));
        assert_eq!(
            update.fields.content,
            Some(vec![ToolCallContent::from("a.rs:1: x".to_string())])
        );
    }

    #[test]
    fn a_finished_command_shows_its_output_and_how_it_ended() {
        let command = |cut, exit| ToolResult::Command {
            head: "start".into(),
            cut,
            tail: "end\n".into(),
            exit,
        };

        assert_eq!(
            finished(command(0, Exit::Code(0))),
            (ToolCallStatus::Completed, "start\n[exit code 0]".into())
        );
        assert_eq!(
            finished(command(5, Exit::Code(2))),
            (
                ToolCallStatus::Failed,
                "start\n[5 bytes cut]\nend\n[exit code 2]".into()
            )
        );
        assert_eq!(
            finished(command(0, Exit::TimedOut { secs: 9 })).1,
            "start\n[timed out after 9 s]"
        );
        assert_eq!(
            finished(ToolResult::Rejected),
            (ToolCallStatus::Failed, "Rejected".into())
        );
    }

    fn write() -> Change {
        Change::WriteFile {
            path: "a.txt".into(),
            old_text: Some("old".into()),
            new_text: "new".into(),
        }
    }

    /// Run a turn whose one tool call asks for `change`; returns the
    /// approval the core got and the permission request the client saw.
    async fn approval(answer: Answer, change: Change) -> (bool, RequestPermissionRequest) {
        let mut test = start(answer).await;
        let pending = test.prompt(&["change"]);
        test.core.next().await;
        test.core
            .send(Event::ApprovalRequested { id: 3, change })
            .await;
        let Command::Approval { id, approve } = test.core.next().await else {
            panic!("expected an approval");
        };
        assert_eq!(id, 3);
        test.core
            .send(Event::TurnEnded {
                stop: StopReason::EndTurn,
            })
            .await;
        response(pending).await.unwrap();
        (approve, test.asked.try_recv().unwrap())
    }

    #[tokio::test]
    async fn a_change_asks_the_client_with_a_diff() {
        let (approve, asked) = approval(Answer::Allow, write()).await;

        assert!(approve);
        assert_eq!(asked.session_id.to_string(), "s1");
        assert_eq!(asked.tool_call.tool_call_id.to_string(), "3");
        assert_eq!(asked.tool_call.fields.title.as_deref(), Some("Write a.txt"));
        assert_eq!(
            asked.tool_call.fields.content,
            Some(vec![ToolCallContent::Diff(
                Diff::new("/work/a.txt", "new").old_text("old".to_string())
            )])
        );
        let options: Vec<_> = asked
            .options
            .iter()
            .map(|option| (option.option_id.to_string(), option.kind))
            .collect();
        assert_eq!(
            options,
            [
                (ALLOW.into(), PermissionOptionKind::AllowOnce),
                (REJECT.into(), PermissionOptionKind::RejectOnce),
            ]
        );
    }

    #[tokio::test]
    async fn a_rejected_cancelled_or_failed_permission_rejects_the_change() {
        for answer in [Answer::Reject, Answer::Cancel, Answer::Fail] {
            assert!(!approval(answer, write()).await.0);
        }
    }

    #[tokio::test]
    async fn a_command_asks_with_its_command_line() {
        let command = Change::RunCommand {
            command: "date".into(),
        };
        let (approve, asked) = approval(Answer::Allow, command).await;

        assert!(approve);
        assert_eq!(asked.tool_call.fields.title.as_deref(), Some("Run date"));
        assert_eq!(asked.tool_call.fields.content, None);
    }

    #[tokio::test]
    async fn cancel_reaches_the_session_and_the_prompt_ends_cancelled() {
        let mut test = start(Answer::Allow).await;
        let pending = test.prompt(&["long"]);
        test.core.next().await;

        let cancel = CancelNotification::new(test.session.clone());
        test.connection.send_notification(cancel).unwrap();
        assert!(matches!(test.core.next().await, Command::Cancel));
        test.core
            .send(Event::TurnEnded {
                stop: StopReason::Cancelled,
            })
            .await;

        assert_eq!(
            response(pending).await.unwrap().stop_reason,
            AcpStop::Cancelled
        );
    }

    #[tokio::test]
    async fn a_second_prompt_during_a_turn_is_an_error_and_the_next_one_works() {
        let mut test = start(Answer::Allow).await;
        let first = test.prompt(&["one"]);
        test.core.next().await;

        let second = response(test.prompt(&["two"])).await.unwrap_err();
        assert_eq!(second.data, Some("a turn is already running".into()));

        test.core
            .send(Event::TurnEnded {
                stop: StopReason::EndTurn,
            })
            .await;
        response(first).await.unwrap();
        let third = test.prompt(&["three"]);
        assert!(matches!(
            test.core.next().await,
            Command::Prompt { text } if text == "three"
        ));
        test.core
            .send(Event::TurnEnded {
                stop: StopReason::EndTurn,
            })
            .await;
        response(third).await.unwrap();
    }

    #[tokio::test]
    async fn an_unknown_session_or_a_non_text_prompt_is_an_error() {
        let mut test = start(Answer::Allow).await;

        test.session = "nope".into();
        let unknown = response(test.prompt(&["hi"])).await.unwrap_err();
        assert_eq!(unknown.code, Error::session_not_found("nope").code);

        test.session = "s1".into();
        let image = ContentBlock::Image(agent_client_protocol::schema::v1::ImageContent::new(
            "AAAA",
            "image/png",
        ));
        let error = response(test.send_prompt(vec![image])).await.unwrap_err();
        assert_eq!(error.data, Some("only text prompts are supported".into()));
    }

    #[tokio::test]
    async fn a_prompt_whose_session_ended_is_an_error_and_drops_the_session() {
        let mut test = start(Answer::Allow).await;
        let pending = test.prompt(&["hi"]);
        test.core.next().await;
        // The core goes away mid-turn: its event sender is dropped.
        let (gone, _) = mpsc::channel(1);
        drop(std::mem::replace(&mut test.core.events, gone));

        let error = response(pending).await.unwrap_err();
        assert_eq!(error.data, Some("the session ended".into()));
        let next = response(test.prompt(&["again"])).await.unwrap_err();
        assert_eq!(next.code, Error::session_not_found("s1").code);
    }
}
