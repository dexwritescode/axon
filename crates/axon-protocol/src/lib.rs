//! Commands and events exchanged between the Axon core and its frontends.
//!
//! Everything here is frontend-agnostic: no model-provider, ACP or UI types.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;

/// Sent by a frontend to a session.
#[derive(Debug)]
pub enum Command {
    /// Run a turn for this user message.
    Prompt { text: String },
    /// Stop the running turn; it ends with `StopReason::Cancelled`.
    /// Does nothing while idle.
    Cancel,
    /// Answer the `ApprovalRequested` event for the tool call with this `id`.
    Approval { id: u64, approve: bool },
}

/// Sent by a session to its frontend.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Event {
    /// The model's context size in tokens, as the server reports it; None when it doesn't. Always the session's first event.
    ContextLimit { tokens: Option<u64> },
    /// A piece of the model's reply, in order.
    TextDelta { text: String },
    /// A piece of the model's reasoning before it replies; not part of the reply.
    ReasoningDelta { text: String },
    /// The model is streaming the arguments of a call to tool `name`; `bytes`
    /// of them so far.
    ToolCallStreaming { name: String, bytes: usize },
    /// The model called a tool; `args` is the JSON text it sent. `id` is
    /// unique within the session and names this call in later events.
    ToolCallStarted { id: u64, name: String, args: String },
    /// The tool call `id` wants to make `change`; it waits for a
    /// `Command::Approval` with the same `id`, or for `Command::Cancel`.
    ApprovalRequested { id: u64, change: Change },
    /// The tool call `id` has finished.
    ToolCallFinished { id: u64, result: ToolResult },
    /// The turn waits up to `secs` seconds (rounded up) before its next
    /// model request.
    Paused { secs: u64 },
    /// Something went wrong; the session stays usable.
    Error { message: String },
    /// The turn started by the last `Prompt` is over.
    TurnEnded { stop: StopReason },
    /// What a resumed session showed before, in order; sent after
    /// `ContextLimit`, only when there is something.
    #[serde(skip)]
    History { items: Vec<Past> },
}

/// A change a tool call wants to make, shown to the user for approval.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Change {
    /// Create or replace the file at `path` (relative to the working
    /// directory); `old_text` is `None` when the file doesn't exist yet.
    WriteFile {
        path: String,
        old_text: Option<String>,
        new_text: String,
    },
    /// Run `command` in the platform shell, in the working directory.
    RunCommand { command: String },
}

/// How a tool call ended.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", content = "value", rename_all = "snake_case")]
pub enum ToolResult {
    /// What a file tool returned.
    Output(String),
    /// The tool couldn't do it; says why.
    Failed(String),
    /// The user rejected the change, or cancelled the turn while asked.
    Rejected,
    /// A command ran: what it printed (stdout and stderr together) and how
    /// it ended. Long output keeps its start and end: `cut` bytes between
    /// `head` and `tail` were left out. When `cut` is 0, `head` holds
    /// everything and `tail` is empty.
    Command {
        head: String,
        cut: usize,
        tail: String,
        exit: Exit,
    },
}

/// How a command ended.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", content = "value", rename_all = "snake_case")]
pub enum Exit {
    Code(i32),
    /// Ended without an exit code, e.g. killed by a signal on Unix.
    Killed,
    /// Killed after running this long.
    TimedOut {
        secs: u64,
    },
    /// Killed because the turn was cancelled.
    Cancelled,
}

/// Why a turn ended.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StopReason {
    /// The model answered without calling more tools, or the turn failed.
    EndTurn,
    /// The model kept calling tools until the per-turn request limit.
    MaxTurnRequests,
    /// The frontend sent `Command::Cancel`.
    Cancelled,
    /// A model reply reached the per-request token limit and was cut off.
    MaxTokens,
}

/// Something a saved session showed, in order: what the user sent, or an
/// event the session sent back.
#[derive(Debug, Clone, PartialEq)]
pub enum Past {
    Prompt(String),
    Event(Event),
}

/// Which session `SessionFactory::new_session` starts.
#[derive(Debug, Clone, PartialEq)]
pub enum Resume {
    /// A new, empty session.
    New,
    /// The newest saved session in the working directory.
    Latest,
    /// The saved session in the working directory whose id starts with this.
    Id(String),
}

/// What a frontend holds: a session's command sender and event receiver.
pub struct SessionHandle {
    /// Names the saved session, for `Resume::Id`.
    pub id: String,
    pub commands: mpsc::Sender<Command>,
    pub events: mpsc::Receiver<Event>,
}

/// Why a `SessionFactory` could not create a session.
#[derive(Debug, thiserror::Error)]
pub enum SessionError {
    /// The session could not be started; the message says why.
    #[error("could not start a session: {0}")]
    Start(String),
    /// There is no saved session to resume.
    #[error("no saved session to resume here")]
    NotFound,
}

/// Creates sessions. Frontends get one from the binary, so they never
/// depend on the core directly.
pub trait SessionFactory: Send + Sync {
    /// Start the session `resume` names, whose tools work inside `cwd`.
    fn new_session(&self, cwd: PathBuf, resume: Resume) -> Result<SessionHandle, SessionError>;
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::de::DeserializeOwned;
    use serde_json::{Value, json};
    use std::fmt::Debug;

    /// Saved sessions depend on these names; a change here breaks them.
    fn same<T: Serialize + DeserializeOwned + PartialEq + Debug>(value: T, expected: Value) {
        assert_eq!(serde_json::to_value(&value).unwrap(), expected);
        assert_eq!(serde_json::from_value::<T>(expected).unwrap(), value);
    }

    #[test]
    fn events_serialize_with_explicit_names() {
        let text = || "hi".to_string();
        same(
            Event::ContextLimit { tokens: Some(8) },
            json!({"type": "context_limit", "tokens": 8}),
        );
        same(
            Event::TextDelta { text: text() },
            json!({"type": "text_delta", "text": "hi"}),
        );
        same(
            Event::ReasoningDelta { text: text() },
            json!({"type": "reasoning_delta", "text": "hi"}),
        );
        same(
            Event::ToolCallStreaming {
                name: text(),
                bytes: 3,
            },
            json!({"type": "tool_call_streaming", "name": "hi", "bytes": 3}),
        );
        same(
            Event::ToolCallStarted {
                id: 1,
                name: text(),
                args: "{}".into(),
            },
            json!({"type": "tool_call_started", "id": 1, "name": "hi", "args": "{}"}),
        );
        same(
            Event::ApprovalRequested {
                id: 1,
                change: Change::RunCommand { command: text() },
            },
            json!({"type": "approval_requested", "id": 1, "change": {"type": "run_command", "command": "hi"}}),
        );
        same(
            Event::ToolCallFinished {
                id: 1,
                result: ToolResult::Rejected,
            },
            json!({"type": "tool_call_finished", "id": 1, "result": {"type": "rejected"}}),
        );
        same(
            Event::Paused { secs: 30 },
            json!({"type": "paused", "secs": 30}),
        );
        same(
            Event::Error { message: text() },
            json!({"type": "error", "message": "hi"}),
        );
        same(
            Event::TurnEnded {
                stop: StopReason::EndTurn,
            },
            json!({"type": "turn_ended", "stop": "end_turn"}),
        );
    }

    #[test]
    fn changes_serialize_with_explicit_names() {
        same(
            Change::WriteFile {
                path: "a.txt".into(),
                old_text: None,
                new_text: "new".into(),
            },
            json!({"type": "write_file", "path": "a.txt", "old_text": null, "new_text": "new"}),
        );
        same(
            Change::RunCommand {
                command: "ls".into(),
            },
            json!({"type": "run_command", "command": "ls"}),
        );
    }

    #[test]
    fn tool_results_serialize_with_explicit_names() {
        same(
            ToolResult::Output("out".into()),
            json!({"type": "output", "value": "out"}),
        );
        same(
            ToolResult::Failed("why".into()),
            json!({"type": "failed", "value": "why"}),
        );
        same(ToolResult::Rejected, json!({"type": "rejected"}));
        same(
            ToolResult::Command {
                head: "a".into(),
                cut: 2,
                tail: "b".into(),
                exit: Exit::Code(1),
            },
            json!({"type": "command", "value": {
                "head": "a", "cut": 2, "tail": "b", "exit": {"type": "code", "value": 1}
            }}),
        );
    }

    #[test]
    fn exits_and_stop_reasons_serialize_with_explicit_names() {
        same(Exit::Code(0), json!({"type": "code", "value": 0}));
        same(Exit::Killed, json!({"type": "killed"}));
        same(
            Exit::TimedOut { secs: 2 },
            json!({"type": "timed_out", "value": {"secs": 2}}),
        );
        same(Exit::Cancelled, json!({"type": "cancelled"}));
        same(StopReason::EndTurn, json!("end_turn"));
        same(StopReason::MaxTurnRequests, json!("max_turn_requests"));
        same(StopReason::Cancelled, json!("cancelled"));
        same(StopReason::MaxTokens, json!("max_tokens"));
    }
}
