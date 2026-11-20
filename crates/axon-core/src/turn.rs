//! The agent loop for one prompt: call the model, run its tool calls, repeat.

use std::collections::HashMap;
use std::path::PathBuf;
use std::pin::pin;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use anyhow::Result;
use async_openai::types::chat::{
    ChatCompletionMessageToolCall, ChatCompletionMessageToolCalls,
    ChatCompletionRequestAssistantMessageArgs, ChatCompletionRequestMessage,
    ChatCompletionRequestSystemMessage, ChatCompletionRequestToolMessageArgs,
    ChatCompletionRequestUserMessage, ChatCompletionTools, FunctionCall,
};
use axon_protocol::{Change, Event, Exit, StopReason, ToolResult};
use futures::StreamExt;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::model::{Delta, ModelClient, RequestPause, ToolCall, ToolCalls};
use crate::tools::{self, Action, Printed, Seen};

const CANCELLED: &str = "cancelled by user";
const REJECTED: &str = "the user rejected this tool call";
const SKIPPED: &str = "not run: the user rejected an earlier tool call";
const EMPTY: &str = "Your last reply was empty: no text and no tool call. \
    If you meant to call a tool, call it now; otherwise give your answer.";

/// The user's answer to an `ApprovalRequested` event.
pub(crate) struct Approval {
    pub(crate) id: u64,
    pub(crate) approve: bool,
}

/// What a turn needs besides the history.
pub(crate) struct Agent {
    pub(crate) client: ModelClient,
    /// Canonical; tools can't reach outside it.
    pub(crate) cwd: PathBuf,
    /// Sent before the history on every request.
    pub(crate) system: String,
    /// The tools offered to the model; it can't run any other.
    pub(crate) tools: Vec<ChatCompletionTools>,
    /// Model requests per prompt before the turn stops.
    pub(crate) max_requests: usize,
    /// Wait between a turn's model requests.
    pub(crate) request_pause: RequestPause,
    /// Each file's content hash as the model last read or wrote it.
    pub(crate) seen: Mutex<HashMap<PathBuf, u64>>,
}

impl Agent {
    /// Run one turn, adding its messages to `history`.
    ///
    /// When `cancel` fires, the turn stops at the next safe point and leaves
    /// `history` valid: a partial reply is kept without its tool calls, and
    /// tool calls not yet run are answered with "cancelled by user".
    /// File changes and commands wait for an answer on `approvals`; a
    /// rejection ends the turn, so the user can say what they want instead.
    /// A reply with neither text nor tool calls is answered once, telling the
    /// model so, before the next request; a second one in a row ends the turn.
    /// Each tool call takes the next id after `last_call`, which the session
    /// keeps across turns so an old answer can't match a new call.
    pub(crate) async fn turn(
        &self,
        history: &mut Vec<ChatCompletionRequestMessage>,
        events: &mpsc::Sender<Event>,
        approvals: &mut mpsc::UnboundedReceiver<Approval>,
        last_call: &mut u64,
        cancel: &CancellationToken,
    ) -> Result<StopReason> {
        let system = ChatCompletionRequestSystemMessage::from(self.system.clone());
        // Pushed only once the next request goes, so a turn stopping first
        // doesn't leave it for the next prompt to follow.
        let mut nudge = false;
        let mut took = None;
        for _ in 0..self.max_requests {
            if let Some(took) = took
                && !self
                    .pause(self.request_pause.after(took), events, cancel)
                    .await?
            {
                return Ok(StopReason::Cancelled);
            }
            if nudge {
                history.push(ChatCompletionRequestUserMessage::from(EMPTY).into());
            }
            let messages = std::iter::once(system.clone().into())
                .chain(history.iter().cloned())
                .collect();
            let began = Instant::now();
            let reply = self.stream_reply(messages, events, cancel).await?;
            took = Some(began.elapsed());
            if reply.cancelled || reply.truncated {
                // Even when empty, so the next prompt doesn't follow a user message.
                history.push(assistant_message(reply.text, &[])?);
                let stop = if reply.cancelled {
                    StopReason::Cancelled
                } else {
                    StopReason::MaxTokens
                };
                return Ok(stop);
            }
            // Reasoning alone isn't an answer: often a tool call the server
            // left in it.
            let empty = reply.calls.is_empty() && reply.text.trim().is_empty();
            history.push(assistant_message(reply.text, &reply.calls)?);
            if empty && !nudge {
                nudge = true;
                continue;
            }
            nudge = false;
            if reply.calls.is_empty() {
                return Ok(StopReason::EndTurn);
            }
            let mut rejected = false;
            for call in reply.calls {
                let output = if cancel.is_cancelled() {
                    CANCELLED.to_string()
                } else if rejected {
                    SKIPPED.to_string()
                } else {
                    *last_call += 1;
                    let asking = Asking {
                        id: *last_call,
                        approvals: &mut *approvals,
                        cancel,
                    };
                    let outcome = self.run_tool(&call, events, asking).await?;
                    rejected = outcome.result == ToolResult::Rejected;
                    outcome.for_model
                };
                history.push(
                    ChatCompletionRequestToolMessageArgs::default()
                        .content(output)
                        .tool_call_id(call.id)
                        .build()?
                        .into(),
                );
            }
            if cancel.is_cancelled() {
                return Ok(StopReason::Cancelled);
            }
            if rejected {
                return Ok(StopReason::EndTurn);
            }
        }
        Ok(StopReason::MaxTurnRequests)
    }

    fn saw(&self, seen: Seen) {
        let mut files = self.seen.lock().expect("seen lock");
        files.insert(seen.target, seen.hash);
    }

    /// Wait `pause`, saying so; false if cancelled meanwhile.
    async fn pause(
        &self,
        pause: Duration,
        events: &mpsc::Sender<Event>,
        cancel: &CancellationToken,
    ) -> Result<bool> {
        if pause.is_zero() {
            return Ok(true);
        }
        // Rounded up, so the wait is never shown as over before it is.
        let secs = pause.as_secs_f64().ceil() as u64;
        events.send(Event::Paused { secs }).await?;
        Ok(tokio::select! {
            biased;
            _ = cancel.cancelled() => false,
            _ = tokio::time::sleep(pause) => true,
        })
    }

    /// Stream one model reply, forwarding its text. On cancel, the request is
    /// dropped (which closes the connection) and the text so far is returned.
    async fn stream_reply(
        &self,
        messages: Vec<ChatCompletionRequestMessage>,
        events: &mpsc::Sender<Event>,
        cancel: &CancellationToken,
    ) -> Result<Reply> {
        let mut reply = Reply::default();
        let stream = tokio::select! {
            biased;
            _ = cancel.cancelled() => {
                reply.cancelled = true;
                return Ok(reply);
            }
            stream = self.client.stream_chat(messages, self.tools.clone()) => stream?,
        };
        let mut stream = pin!(stream);
        let mut calls = ToolCalls::default();
        loop {
            let delta = tokio::select! {
                biased;
                _ = cancel.cancelled() => {
                    reply.cancelled = true;
                    return Ok(reply);
                }
                delta = stream.next() => match delta {
                    Some(delta) => delta?,
                    None => break,
                },
            };
            match delta {
                Delta::Text(delta) => {
                    reply.text.push_str(&delta);
                    events.send(Event::TextDelta { text: delta }).await?;
                }
                Delta::Reasoning(text) => {
                    events.send(Event::ReasoningDelta { text }).await?;
                }
                Delta::ToolCall(chunk) => {
                    if let Some((name, bytes)) = calls.push(chunk) {
                        let name = name.to_string();
                        events
                            .send(Event::ToolCallStreaming { name, bytes })
                            .await?;
                    }
                }
                Delta::Truncated => reply.truncated = true,
            }
        }
        reply.calls = calls.finish();
        Ok(reply)
    }

    /// Whether the model was offered the tool `name`.
    fn offers(&self, name: &str) -> bool {
        self.tools.iter().any(|tool| tools::name(tool) == name)
    }

    /// Run tool call `id`, reporting it to the frontend and asking before
    /// any file change or command; returns how it ended for the model.
    async fn run_tool(
        &self,
        call: &ToolCall,
        events: &mpsc::Sender<Event>,
        mut asking: Asking<'_>,
    ) -> Result<Outcome> {
        let id = asking.id;
        events
            .send(Event::ToolCallStarted {
                id,
                name: call.name.clone(),
                args: call.args.clone(),
            })
            .await?;
        let action = if self.offers(&call.name) {
            let (cwd, name, args) = (self.cwd.clone(), call.name.clone(), call.args.clone());
            tokio::task::spawn_blocking(move || tools::run(&cwd, &name, &args)).await?
        } else {
            Err(format!("unknown tool: {}", call.name))
        };
        let outcome = match action {
            Ok(Action::Output(output)) => Outcome::output(output),
            Ok(Action::Read { output, seen }) => {
                self.saw(seen);
                Outcome::output(output)
            }
            Ok(Action::Write(write)) => {
                let checked = write.check(&self.seen.lock().expect("seen lock"));
                match checked {
                    Err(error) => Outcome::failed(error),
                    Ok(()) => match asking.ask(write.change(), events).await? {
                        Some(true) => {
                            let seen = write.seen_after();
                            match tokio::task::spawn_blocking(move || write.apply()).await? {
                                Ok(output) => {
                                    self.saw(seen);
                                    Outcome::output(output)
                                }
                                Err(error) => Outcome::failed(error),
                            }
                        }
                        answer => Outcome::not_approved(answer),
                    },
                }
            }
            Ok(Action::Run(run)) => match asking.ask(run.change(), events).await? {
                Some(true) => match run.execute(&self.cwd, asking.cancel).await {
                    Ok((printed, exit)) => Outcome::command(printed, exit),
                    Err(error) => Outcome::failed(error),
                },
                answer => Outcome::not_approved(answer),
            },
            Err(error) => Outcome::failed(error),
        };
        events
            .send(Event::ToolCallFinished {
                id,
                result: outcome.result.clone(),
            })
            .await?;
        Ok(outcome)
    }
}

/// How a tool call ended: for the frontend, and for the model.
struct Outcome {
    result: ToolResult,
    for_model: String,
}

impl Outcome {
    fn output(output: String) -> Self {
        Self {
            for_model: output.clone(),
            result: ToolResult::Output(output),
        }
    }

    fn failed(error: String) -> Self {
        Self {
            for_model: format!("error: {error}"),
            result: ToolResult::Failed(error),
        }
    }

    /// The user said no (`Some(false)`) or cancelled while asked (`None`).
    fn not_approved(answer: Option<bool>) -> Self {
        Self {
            result: ToolResult::Rejected,
            for_model: if answer.is_some() {
                REJECTED
            } else {
                CANCELLED
            }
            .into(),
        }
    }

    fn command(Printed { head, cut, tail }: Printed, exit: Exit) -> Self {
        let ending = match exit {
            Exit::Code(code) => format!("exit code {code}"),
            Exit::Killed => "killed by a signal".into(),
            Exit::TimedOut { secs } => format!("timed out after {secs} s and was killed"),
            Exit::Cancelled => CANCELLED.into(),
        };
        let printed = match cut {
            0 => head.clone(),
            cut => format!("{head}\n[… {cut} bytes cut …]\n{tail}"),
        };
        let printed = match printed.trim_end_matches('\n') {
            "" => "(no output)",
            printed => printed,
        };
        Self {
            for_model: format!("{printed}\n[{ending}]"),
            result: ToolResult::Command {
                head,
                cut,
                tail,
                exit,
            },
        }
    }
}

/// One tool call that may ask the user: its id and where answers come from.
struct Asking<'a> {
    id: u64,
    approvals: &'a mut mpsc::UnboundedReceiver<Approval>,
    cancel: &'a CancellationToken,
}

impl Asking<'_> {
    /// Show `change` and wait for the user: their answer, or `None` if the
    /// turn is cancelled first. Answers to other calls are ignored.
    async fn ask(&mut self, change: Change, events: &mpsc::Sender<Event>) -> Result<Option<bool>> {
        events
            .send(Event::ApprovalRequested {
                id: self.id,
                change,
            })
            .await?;
        loop {
            tokio::select! {
                biased;
                _ = self.cancel.cancelled() => return Ok(None),
                approval = self.approvals.recv() => match approval {
                    Some(approval) if approval.id == self.id => return Ok(Some(approval.approve)),
                    Some(_) => {}
                    None => return Ok(None),
                },
            }
        }
    }
}

#[derive(Default)]
struct Reply {
    text: String,
    calls: Vec<ToolCall>,
    cancelled: bool,
    /// Cut off at the token limit; its tool calls are incomplete.
    truncated: bool,
}

fn assistant_message(text: String, calls: &[ToolCall]) -> Result<ChatCompletionRequestMessage> {
    let mut message = ChatCompletionRequestAssistantMessageArgs::default();
    if !text.is_empty() || calls.is_empty() {
        message.content(text);
    }
    if !calls.is_empty() {
        message.tool_calls(
            calls
                .iter()
                .map(|call| {
                    ChatCompletionMessageToolCalls::Function(ChatCompletionMessageToolCall {
                        id: call.id.clone(),
                        function: FunctionCall {
                            name: call.name.clone(),
                            arguments: call.args.clone(),
                        },
                    })
                })
                .collect::<Vec<_>>(),
        );
    }
    Ok(message.build()?.into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{
        agent, chunk, client_error, delta, end, prompt, prompt_answering, reply, respond,
        sent_messages, spawn_started, sse, tool_calls,
    };
    use axon_protocol::SessionHandle;
    use axon_protocol::{Change, Command};
    use serde_json::json;
    use std::fs;
    use std::time::Duration;
    use tempfile::TempDir;
    use tokio::sync::mpsc;
    use wiremock::MockServer;

    /// A session in a directory holding notes.txt, which the model has read.
    async fn session_with_notes(max_requests: usize) -> (MockServer, TempDir, SessionHandle) {
        let server = MockServer::start().await;
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join("notes.txt"), "remember the milk\n").unwrap();
        let agent = Agent {
            max_requests,
            ..agent(&server, dir.path())
        };
        has_read(&agent, "notes.txt");
        let handle = spawn_started(agent).await;
        (server, dir, handle)
    }

    fn has_read(agent: &Agent, path: &str) {
        let args = json!({ "path": path }).to_string();
        let Ok(Action::Read { seen, .. }) = tools::run(&agent.cwd, "read_file", &args) else {
            panic!("couldn't read {path}");
        };
        agent.saw(seen);
    }

    #[tokio::test]
    async fn changing_a_file_the_model_has_not_read_fails_without_asking() {
        let server = MockServer::start().await;
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join("notes.txt"), "remember the milk\n").unwrap();
        let mut handle = spawn_started(agent(&server, dir.path())).await;
        let args = r#"{"path":"notes.txt","content":"tea\n"}"#;
        respond(&server, tool_calls(&[("c1", "write_file", args)])).await;
        respond(&server, reply(&["Ok."])).await;

        let events = prompt(&mut handle, "write").await;

        assert!(
            !events
                .iter()
                .any(|e| matches!(e, Event::ApprovalRequested { .. })),
            "{events:?}"
        );
        assert_eq!(
            sent_messages(&server).await[1][2]["content"],
            "error: notes.txt: read it before changing it"
        );
    }

    #[tokio::test]
    async fn changing_a_file_that_changed_since_it_was_read_fails_without_asking() {
        let (server, dir, mut handle) = session_with_notes(25).await;
        fs::write(dir.path().join("notes.txt"), "remember the bread\n").unwrap();
        let args = r#"{"path":"notes.txt","old_string":"bread","new_string":"eggs"}"#;
        respond(&server, tool_calls(&[("c1", "edit_file", args)])).await;
        respond(&server, reply(&["Ok."])).await;

        let events = prompt(&mut handle, "edit").await;

        assert!(
            !events
                .iter()
                .any(|e| matches!(e, Event::ApprovalRequested { .. })),
            "{events:?}"
        );
        assert_eq!(
            sent_messages(&server).await[1][2]["content"],
            "error: notes.txt: changed since you last read it; read it again before changing it"
        );
    }

    #[tokio::test]
    async fn a_file_the_model_wrote_can_be_changed_again() {
        let (server, dir, mut handle) = session_with_notes(25).await;
        let first = r#"{"path":"notes.txt","old_string":"milk","new_string":"eggs"}"#;
        let second = r#"{"path":"notes.txt","old_string":"eggs","new_string":"tea"}"#;
        respond(
            &server,
            tool_calls(&[("c1", "edit_file", first), ("c2", "edit_file", second)]),
        )
        .await;
        respond(&server, reply(&["Done."])).await;

        prompt_answering(&mut handle, "edit twice", Some(true)).await;

        assert_eq!(
            fs::read_to_string(dir.path().join("notes.txt")).unwrap(),
            "remember the tea\n"
        );
    }

    #[tokio::test]
    async fn tool_result_goes_back_to_the_model_which_then_answers() {
        let (server, _dir, mut handle) = session_with_notes(25).await;
        let args = r#"{"path":"notes.txt"}"#;
        respond(&server, tool_calls(&[("call_1", "read_file", args)])).await;
        respond(&server, reply(&["Milk."])).await;

        assert_eq!(
            prompt(&mut handle, "what's in my notes?").await,
            [
                Event::ToolCallStarted {
                    id: 1,
                    name: "read_file".into(),
                    args: args.into()
                },
                finished(1, ToolResult::Output("remember the milk\n".into())),
                delta("Milk."),
                end(StopReason::EndTurn),
            ]
        );
        assert_eq!(
            sent_messages(&server).await[1],
            json!([
                {"role": "user", "content": "what's in my notes?"},
                {"role": "assistant", "tool_calls": [{"id": "call_1", "type": "function",
                    "function": {"name": "read_file", "arguments": args}}]},
                {"role": "tool", "content": "remember the milk\n", "tool_call_id": "call_1"}
            ])
        );
    }

    #[tokio::test]
    async fn every_request_starts_with_a_system_prompt_naming_the_cwd() {
        let (server, dir, mut handle) = session_with_notes(25).await;
        respond(&server, reply(&["Hi"])).await;

        prompt(&mut handle, "hi").await;

        let request = &server.received_requests().await.unwrap()[0];
        let body: serde_json::Value = request.body_json().unwrap();
        let cwd = dir.path().canonicalize().unwrap();
        assert!(
            body["messages"][0]["content"]
                .as_str()
                .unwrap()
                .contains(cwd.to_str().unwrap())
        );
        assert_eq!(
            body["tools"].as_array().unwrap().len(),
            tools::schemas().len()
        );
    }

    #[tokio::test]
    async fn failing_tool_reports_the_error_to_frontend_and_model() {
        let (server, _dir, mut handle) = session_with_notes(25).await;
        respond(&server, tool_calls(&[("c1", "read_file", "{not json}")])).await;
        respond(&server, reply(&["Sorry."])).await;

        let events = prompt(&mut handle, "read").await;

        let Event::ToolCallFinished {
            result: ToolResult::Failed(output),
            ..
        } = &events[1]
        else {
            panic!("{events:?}");
        };
        assert!(
            output.starts_with("arguments were not valid JSON"),
            "{output}"
        );
        assert_eq!(
            sent_messages(&server).await[1][2]["content"],
            format!("error: {output}")
        );
    }

    #[tokio::test]
    async fn a_tool_the_model_was_not_offered_is_refused() {
        let server = MockServer::start().await;
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join("notes.txt"), "secret\n").unwrap();
        let mut handle = spawn_started(Agent {
            tools: tools::enabled(Some(&["grep".into()])),
            ..agent(&server, dir.path())
        })
        .await;
        let args = r#"{"path":"notes.txt"}"#;
        respond(&server, tool_calls(&[("c1", "read_file", args)])).await;
        respond(&server, reply(&["Sorry."])).await;

        let events = prompt(&mut handle, "read").await;

        assert_eq!(
            events[1],
            Event::ToolCallFinished {
                id: 1,
                result: ToolResult::Failed("unknown tool: read_file".into()),
            }
        );
        assert_eq!(
            sent_messages(&server).await[1][2]["content"],
            "error: unknown tool: read_file"
        );
    }

    #[tokio::test]
    async fn turn_stops_at_the_request_limit() {
        let (server, _dir, mut handle) = session_with_notes(2).await;
        for _ in 0..3 {
            respond(&server, tool_calls(&[("c", "list_dir", "{}")])).await;
        }

        let events = prompt(&mut handle, "loop").await;

        assert_eq!(events.last(), Some(&end(StopReason::MaxTurnRequests)));
        assert_eq!(server.received_requests().await.unwrap().len(), 2);
    }

    #[tokio::test]
    async fn a_ratio_pause_lasts_in_proportion_to_the_request_before() {
        let server = MockServer::start().await;
        let dir = TempDir::new().unwrap();
        let mut handle = spawn_started(Agent {
            request_pause: RequestPause::Ratio {
                ratio: 2.0,
                max: None,
            },
            ..agent(&server, dir.path())
        })
        .await;
        let slow = tool_calls(&[("c", "list_dir", "{}")]).set_delay(Duration::from_millis(600));
        respond(&server, slow).await;
        respond(&server, reply(&["done"])).await;

        let events = prompt(&mut handle, "list").await;

        assert!(events.contains(&Event::Paused { secs: 2 }), "{events:?}");
    }

    #[tokio::test]
    async fn requests_after_the_first_wait_for_the_pause() {
        let server = MockServer::start().await;
        let dir = TempDir::new().unwrap();
        let mut handle = spawn_started(Agent {
            request_pause: RequestPause::Fixed(Duration::from_millis(300)),
            ..agent(&server, dir.path())
        })
        .await;
        respond(&server, tool_calls(&[("c", "list_dir", "{}")])).await;
        respond(&server, reply(&["done"])).await;

        let started = std::time::Instant::now();
        let events = prompt(&mut handle, "list").await;

        assert!(started.elapsed() >= Duration::from_millis(300));
        let pauses = events.iter().filter(|e| matches!(e, Event::Paused { .. }));
        assert_eq!(pauses.collect::<Vec<_>>(), [&Event::Paused { secs: 1 }]);
        assert_eq!(events.last(), Some(&end(StopReason::EndTurn)));
    }

    #[tokio::test]
    async fn a_reply_cut_at_the_token_limit_ends_the_turn_without_its_tool_calls() {
        let server = MockServer::start().await;
        let chunks = [
            chunk(json!({"content": "Let me look."}), None),
            chunk(
                json!({"tool_calls": [{"index": 0, "id": "c", "type": "function", "function": {"name": "list_dir", "arguments": "{\"pa"}}]}),
                Some("length"),
            ),
        ];
        respond(&server, sse(&chunks)).await;
        let dir = TempDir::new().unwrap();
        let mut handle = spawn_started(agent(&server, dir.path())).await;

        assert_eq!(
            prompt(&mut handle, "hi").await,
            [delta("Let me look."), end(StopReason::MaxTokens)]
        );
    }

    #[tokio::test]
    async fn reasoning_and_tool_call_progress_reach_the_frontend() {
        let server = MockServer::start().await;
        let dir = TempDir::new().unwrap();
        respond(
            &server,
            sse(&[
                chunk(json!({"reasoning_content": "Let me list."}), None),
                chunk(json!({"content": "Listed."}), None),
            ]),
        )
        .await;
        let sessions = TempDir::new().unwrap();
        let store = crate::store::Store::new(sessions.path(), dir.path(), "test-model");
        let mut handle = crate::session::spawn(
            agent(&server, dir.path()),
            async { None },
            store,
            Default::default(),
        );
        handle.events.recv().await;
        handle
            .commands
            .send(Command::Prompt { text: "hi".into() })
            .await
            .unwrap();
        assert_eq!(
            handle.events.recv().await,
            Some(Event::ReasoningDelta {
                text: "Let me list.".into()
            })
        );

        respond(&server, tool_calls(&[("c", "list_dir", "{}")])).await;
        handle
            .commands
            .send(Command::Prompt {
                text: "list".into(),
            })
            .await
            .unwrap();
        let mut streaming = Vec::new();
        while let Some(event) = handle.events.recv().await {
            match event {
                Event::ToolCallStreaming { name, bytes } => streaming.push((name, bytes)),
                Event::ToolCallStarted { .. } => break,
                _ => {}
            }
        }
        assert_eq!(
            streaming,
            [("list_dir".to_string(), 1), ("list_dir".to_string(), 2)]
        );
    }

    /// The roles and contents of the messages in the `n`th request.
    async fn sent(server: &MockServer, n: usize) -> Vec<(String, String)> {
        sent_messages(server).await[n]
            .as_array()
            .unwrap()
            .iter()
            .map(|m| {
                let content = m["content"].as_str().unwrap_or_default();
                (m["role"].as_str().unwrap().into(), content.into())
            })
            .collect()
    }

    fn msgs(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
        pairs
            .iter()
            .map(|(r, c)| (r.to_string(), c.to_string()))
            .collect()
    }

    #[tokio::test]
    async fn an_empty_reply_is_answered_once_instead_of_ending_the_turn() {
        let server = MockServer::start().await;
        let dir = TempDir::new().unwrap();
        let thinking = [chunk(json!({"reasoning_content": "<tool_call>"}), None)];
        respond(&server, sse(&thinking)).await;
        respond(&server, reply(&["Done."])).await;
        let mut handle = spawn_started(agent(&server, dir.path())).await;

        let events = prompt(&mut handle, "go").await;

        assert_eq!(events.last(), Some(&end(StopReason::EndTurn)));
        assert_eq!(
            sent(&server, 1).await,
            msgs(&[("user", "go"), ("assistant", ""), ("user", EMPTY)])
        );
    }

    #[tokio::test]
    async fn a_second_empty_reply_in_a_row_ends_the_turn() {
        let server = MockServer::start().await;
        let dir = TempDir::new().unwrap();
        respond(&server, reply(&["  "])).await;
        respond(&server, reply(&[])).await;
        respond(&server, reply(&["ok"])).await;
        let mut handle = spawn_started(agent(&server, dir.path())).await;

        let events = prompt(&mut handle, "go").await;
        prompt(&mut handle, "again").await;

        assert_eq!(events.last(), Some(&end(StopReason::EndTurn)));
        assert_eq!(
            sent(&server, 2).await,
            msgs(&[
                ("user", "go"),
                ("assistant", "  "),
                ("user", EMPTY),
                ("assistant", ""),
                ("user", "again"),
            ])
        );
    }

    #[tokio::test]
    async fn a_turn_out_of_requests_after_an_empty_reply_leaves_no_answer_to_it() {
        let (server, _dir, mut handle) = session_with_notes(1).await;
        respond(&server, reply(&[])).await;
        respond(&server, reply(&["ok"])).await;

        let events = prompt(&mut handle, "go").await;
        prompt(&mut handle, "again").await;

        assert_eq!(events.last(), Some(&end(StopReason::MaxTurnRequests)));
        assert_eq!(
            sent(&server, 1).await,
            msgs(&[("user", "go"), ("assistant", ""), ("user", "again")])
        );
    }

    #[tokio::test]
    async fn a_turn_cancelled_in_the_pause_after_an_empty_reply_leaves_no_answer_to_it() {
        let server = MockServer::start().await;
        let dir = TempDir::new().unwrap();
        let mut handle = spawn_started(Agent {
            request_pause: RequestPause::Fixed(Duration::from_secs(60)),
            ..agent(&server, dir.path())
        })
        .await;
        respond(&server, reply(&[])).await;
        handle
            .commands
            .send(Command::Prompt { text: "go".into() })
            .await
            .unwrap();
        while let Some(event) = handle.events.recv().await {
            if event == (Event::Paused { secs: 60 }) {
                break;
            }
        }
        handle.commands.send(Command::Cancel).await.unwrap();
        while let Some(event) = handle.events.recv().await {
            if matches!(event, Event::TurnEnded { .. }) {
                break;
            }
        }
        respond(&server, reply(&["ok"])).await;

        prompt(&mut handle, "again").await;

        assert_eq!(
            sent(&server, 1).await,
            msgs(&[("user", "go"), ("assistant", ""), ("user", "again")])
        );
    }

    #[tokio::test]
    async fn a_reply_cut_before_any_text_still_answers_the_prompt() {
        let server = MockServer::start().await;
        let dir = TempDir::new().unwrap();
        let chunks = [chunk(json!({"reasoning_content": "Hmm"}), Some("length"))];
        respond(&server, sse(&chunks)).await;
        respond(&server, reply(&["ok"])).await;
        let mut handle = spawn_started(agent(&server, dir.path())).await;

        prompt(&mut handle, "think hard").await;
        prompt(&mut handle, "again").await;

        let roles: Vec<_> = sent_messages(&server).await[1]
            .as_array()
            .unwrap()
            .iter()
            .map(|m| m["role"].clone())
            .collect();
        assert_eq!(roles, ["user", "assistant", "user"]);
    }

    #[tokio::test]
    async fn cancel_cuts_a_pause_short() {
        let server = MockServer::start().await;
        let dir = TempDir::new().unwrap();
        let mut handle = spawn_started(Agent {
            request_pause: RequestPause::Fixed(Duration::from_secs(60)),
            ..agent(&server, dir.path())
        })
        .await;
        respond(&server, tool_calls(&[("c", "list_dir", "{}")])).await;
        handle
            .commands
            .send(Command::Prompt {
                text: "list".into(),
            })
            .await
            .unwrap();

        while let Some(event) = handle.events.recv().await {
            if event == (Event::Paused { secs: 60 }) {
                break;
            }
        }
        handle.commands.send(Command::Cancel).await.unwrap();
        let ended = tokio::time::timeout(Duration::from_secs(5), handle.events.recv()).await;

        assert_eq!(ended.unwrap(), Some(end(StopReason::Cancelled)));
        assert_eq!(server.received_requests().await.unwrap().len(), 1);
    }

    #[tokio::test]
    async fn failed_turn_leaves_no_partial_messages_in_history() {
        let (server, _dir, mut handle) = session_with_notes(25).await;
        respond(&server, tool_calls(&[("c", "list_dir", "{}")])).await;
        respond(&server, client_error()).await;
        respond(&server, reply(&["ok"])).await;

        let events = prompt(&mut handle, "first").await;
        assert!(matches!(events[2], Event::Error { .. }), "{events:?}");
        assert_eq!(events[3..], [end(StopReason::EndTurn)]);
        prompt(&mut handle, "second").await;

        assert_eq!(
            sent_messages(&server).await[2],
            json!([{"role": "user", "content": "second"}])
        );
    }

    /// Run a turn directly with a one-slot event channel, cancelling as soon
    /// as the first event arrives. Returns the stop reason, the events and
    /// the history.
    async fn cancel_after_first_event(
        agent: &Agent,
    ) -> (StopReason, Vec<Event>, Vec<ChatCompletionRequestMessage>) {
        let (events, mut received) = mpsc::channel(1);
        let cancel = CancellationToken::new();
        let mut history = vec![ChatCompletionRequestMessage::User("go".into())];
        let (_approve, mut approvals) = mpsc::unbounded_channel();
        let mut last_call = 0;
        let mut seen = Vec::new();
        let stop = {
            let mut turn = pin!(agent.turn(
                &mut history,
                &events,
                &mut approvals,
                &mut last_call,
                &cancel
            ));
            loop {
                tokio::select! {
                    stop = &mut turn => break stop.unwrap(),
                    Some(event) = received.recv() => {
                        if !matches!(event, Event::ToolCallStreaming { .. }) {
                            seen.push(event);
                            cancel.cancel();
                        }
                    }
                }
            }
        };
        while let Ok(event) = received.try_recv() {
            if !matches!(event, Event::ToolCallStreaming { .. }) {
                seen.push(event);
            }
        }
        (stop, seen, history)
    }

    #[tokio::test]
    async fn cancel_mid_stream_keeps_the_text_shown_so_far() {
        let server = MockServer::start().await;
        let deltas: Vec<String> = (0..10).map(|n| format!("{n} ")).collect();
        let deltas: Vec<&str> = deltas.iter().map(String::as_str).collect();
        respond(&server, reply(&deltas)).await;
        let dir = TempDir::new().unwrap();

        let (stop, seen, history) = cancel_after_first_event(&agent(&server, dir.path())).await;

        assert_eq!(stop, StopReason::Cancelled);
        let shown: String = seen
            .iter()
            .map(|event| match event {
                Event::TextDelta { text } => text.as_str(),
                other => panic!("{other:?}"),
            })
            .collect();
        assert!(seen.len() < deltas.len(), "{seen:?}");
        let last = serde_json::to_value(history.last().unwrap()).unwrap();
        assert_eq!(last, json!({"role": "assistant", "content": shown}));
    }

    #[tokio::test]
    async fn cancel_between_tool_calls_answers_the_rest_as_cancelled() {
        let (server, dir, _handle) = session_with_notes(25).await;
        respond(
            &server,
            tool_calls(&[
                ("c1", "read_file", r#"{"path":"notes.txt"}"#),
                ("c2", "list_dir", "{}"),
            ]),
        )
        .await;

        let (stop, _seen, history) = cancel_after_first_event(&agent(&server, dir.path())).await;

        assert_eq!(stop, StopReason::Cancelled);
        let tool_results: Vec<_> = history[2..]
            .iter()
            .map(|message| serde_json::to_value(message).unwrap())
            .map(|message| (message["tool_call_id"].clone(), message["content"].clone()))
            .collect();
        assert_eq!(
            tool_results,
            [
                (json!("c1"), json!("remember the milk\n")),
                (json!("c2"), json!("cancelled by user"))
            ]
        );
        assert_eq!(server.received_requests().await.unwrap().len(), 1);
    }

    fn finished(id: u64, result: ToolResult) -> Event {
        Event::ToolCallFinished { id, result }
    }

    #[tokio::test]
    async fn approved_write_creates_the_file_and_tells_the_model() {
        let (server, dir, mut handle) = session_with_notes(25).await;
        let args = r#"{"path":"sub/new.txt","content":"hi\n"}"#;
        respond(&server, tool_calls(&[("c1", "write_file", args)])).await;
        respond(&server, reply(&["Done."])).await;

        let events = prompt_answering(&mut handle, "write", Some(true)).await;

        assert_eq!(
            events[1..3],
            [
                Event::ApprovalRequested {
                    id: 1,
                    change: Change::WriteFile {
                        path: "sub/new.txt".into(),
                        old_text: None,
                        new_text: "hi\n".into()
                    }
                },
                finished(1, ToolResult::Output("wrote sub/new.txt".into())),
            ]
        );
        assert_eq!(
            fs::read_to_string(dir.path().join("sub/new.txt")).unwrap(),
            "hi\n"
        );
        assert_eq!(
            sent_messages(&server).await[1][2]["content"],
            "wrote sub/new.txt"
        );
    }

    #[tokio::test]
    async fn approved_edit_changes_the_file() {
        let (server, dir, mut handle) = session_with_notes(25).await;
        let args = r#"{"path":"notes.txt","old_string":"milk","new_string":"eggs"}"#;
        respond(&server, tool_calls(&[("c1", "edit_file", args)])).await;
        respond(&server, reply(&["Done."])).await;

        let events = prompt_answering(&mut handle, "edit", Some(true)).await;

        assert_eq!(
            events[1],
            Event::ApprovalRequested {
                id: 1,
                change: Change::WriteFile {
                    path: "notes.txt".into(),
                    old_text: Some("remember the milk\n".into()),
                    new_text: "remember the eggs\n".into()
                }
            }
        );
        assert_eq!(
            fs::read_to_string(dir.path().join("notes.txt")).unwrap(),
            "remember the eggs\n"
        );
    }

    #[tokio::test]
    async fn rejection_leaves_the_file_skips_the_rest_and_ends_the_turn() {
        let (server, dir, mut handle) = session_with_notes(25).await;
        let args = r#"{"path":"notes.txt","old_string":"milk","new_string":"eggs"}"#;
        respond(
            &server,
            tool_calls(&[("c1", "edit_file", args), ("c2", "list_dir", "{}")]),
        )
        .await;
        respond(&server, reply(&["Ok."])).await;

        let events = prompt_answering(&mut handle, "edit", Some(false)).await;

        assert_eq!(
            events[2..],
            [finished(1, ToolResult::Rejected), end(StopReason::EndTurn)]
        );
        assert_eq!(
            fs::read_to_string(dir.path().join("notes.txt")).unwrap(),
            "remember the milk\n"
        );
        assert_eq!(server.received_requests().await.unwrap().len(), 1);

        prompt(&mut handle, "use bread instead").await;
        let sent = &sent_messages(&server).await[1];
        assert_eq!(sent[2]["content"], REJECTED);
        assert_eq!(sent[3]["content"], SKIPPED);
        assert_eq!(sent[4]["content"], "use bread instead");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn approved_command_runs_and_the_model_gets_output_and_exit_code() {
        let (server, _dir, mut handle) = session_with_notes(25).await;
        let args = r#"{"command":"cat notes.txt; exit 2"}"#;
        respond(&server, tool_calls(&[("c1", "run_command", args)])).await;
        respond(&server, reply(&["Done."])).await;

        let events = prompt_answering(&mut handle, "run", Some(true)).await;

        assert_eq!(
            events[1..3],
            [
                Event::ApprovalRequested {
                    id: 1,
                    change: Change::RunCommand {
                        command: "cat notes.txt; exit 2".into()
                    }
                },
                finished(
                    1,
                    ToolResult::Command {
                        head: "remember the milk\n".into(),
                        cut: 0,
                        tail: String::new(),
                        exit: Exit::Code(2)
                    }
                ),
            ]
        );
        assert_eq!(
            sent_messages(&server).await[1][2]["content"],
            "remember the milk\n[exit code 2]"
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn cancel_kills_a_running_command_and_ends_the_turn() {
        let (server, _dir, mut handle) = session_with_notes(25).await;
        let args = r#"{"command":"sleep 30"}"#;
        respond(&server, tool_calls(&[("c1", "run_command", args)])).await;
        handle
            .commands
            .send(Command::Prompt { text: "run".into() })
            .await
            .unwrap();

        let events = tokio::time::timeout(Duration::from_secs(5), async {
            let mut events = Vec::new();
            loop {
                let event = handle.events.recv().await.unwrap();
                if let Event::ApprovalRequested { id, .. } = event {
                    let approve = Command::Approval { id, approve: true };
                    handle.commands.send(approve).await.unwrap();
                    tokio::time::sleep(Duration::from_millis(100)).await;
                    handle.commands.send(Command::Cancel).await.unwrap();
                }
                let done = matches!(event, Event::TurnEnded { .. });
                events.push(event);
                if done {
                    break events;
                }
            }
        })
        .await
        .expect("cancel should kill the command");
        assert_eq!(
            events[2..],
            [
                finished(
                    1,
                    ToolResult::Command {
                        head: String::new(),
                        cut: 0,
                        tail: String::new(),
                        exit: Exit::Cancelled
                    }
                ),
                end(StopReason::Cancelled)
            ]
        );
    }

    #[test]
    fn the_model_hears_how_a_command_ended() {
        let whole = |head: &str| Printed {
            head: head.into(),
            cut: 0,
            tail: String::new(),
        };
        let heard = |head: &str, exit| Outcome::command(whole(head), exit).for_model;

        assert_eq!(heard("ok\n", Exit::Code(0)), "ok\n[exit code 0]");
        assert_eq!(heard("", Exit::Killed), "(no output)\n[killed by a signal]");
        assert_eq!(
            heard("x", Exit::TimedOut { secs: 5 }),
            "x\n[timed out after 5 s and was killed]"
        );
        assert_eq!(
            heard("", Exit::Cancelled),
            format!("(no output)\n[{CANCELLED}]")
        );
        let long = Printed {
            head: "start\n".into(),
            cut: 100,
            tail: "end\n".into(),
        };
        assert_eq!(
            Outcome::command(long, Exit::Code(0)).for_model,
            "start\n\n[… 100 bytes cut …]\nend\n[exit code 0]"
        );
    }

    #[tokio::test]
    async fn invalid_change_fails_without_asking() {
        let (server, _dir, mut handle) = session_with_notes(25).await;
        let args = r#"{"path":"../escape.txt","content":"x"}"#;
        respond(&server, tool_calls(&[("c1", "write_file", args)])).await;
        respond(&server, reply(&["Ok."])).await;

        let events = prompt(&mut handle, "write").await;

        assert!(
            matches!(&events[1], Event::ToolCallFinished { result: ToolResult::Failed(error), .. } if error.contains("outside")),
            "{events:?}"
        );
    }

    #[tokio::test]
    async fn tool_call_ids_keep_counting_across_turns() {
        let (server, _dir, mut handle) = session_with_notes(25).await;
        for path in ["a.txt", "b.txt"] {
            let args = format!(r#"{{"path":"{path}","content":"x"}}"#);
            respond(&server, tool_calls(&[("c1", "write_file", &args)])).await;
            respond(&server, reply(&["Done."])).await;
        }

        let first = prompt_answering(&mut handle, "write a", Some(true)).await;
        let second = prompt_answering(&mut handle, "write b", Some(true)).await;

        let id = |events: &[Event]| {
            events.iter().find_map(|event| match event {
                Event::ApprovalRequested { id, .. } => Some(*id),
                _ => None,
            })
        };
        assert_eq!((id(&first), id(&second)), (Some(1), Some(2)));
    }

    #[tokio::test]
    async fn answers_to_other_requests_are_ignored_and_cancel_rejects() {
        let (server, dir, mut handle) = session_with_notes(25).await;
        let args = r#"{"path":"new.txt","content":"x"}"#;
        respond(&server, tool_calls(&[("c1", "write_file", args)])).await;
        let commands = handle.commands.clone();
        commands
            .send(Command::Prompt {
                text: "write".into(),
            })
            .await
            .unwrap();
        commands
            .send(Command::Approval {
                id: 99,
                approve: true,
            })
            .await
            .unwrap();

        let events = tokio::time::timeout(Duration::from_secs(5), async {
            let mut events = Vec::new();
            loop {
                let event = handle.events.recv().await.unwrap();
                if matches!(event, Event::ApprovalRequested { .. }) {
                    handle.commands.send(Command::Cancel).await.unwrap();
                }
                let done = matches!(event, Event::TurnEnded { .. });
                events.push(event);
                if done {
                    break events;
                }
            }
        })
        .await
        .expect("cancel should end the wait");
        assert!(
            matches!(events[1], Event::ApprovalRequested { id: 1, .. }),
            "{events:?}"
        );
        assert_eq!(
            events[2..],
            [
                finished(1, ToolResult::Rejected),
                end(StopReason::Cancelled)
            ]
        );
        assert!(!dir.path().join("new.txt").exists());

        respond(&server, reply(&["Ok."])).await;
        prompt(&mut handle, "never mind").await;
        let sent = &sent_messages(&server).await[1];
        assert_eq!(sent[2]["content"], CANCELLED);
    }
}
