//! Commands and events exchanged between the Axon core and its frontends.
//!
//! Everything here is frontend-agnostic: no model-provider, ACP or UI types.

use std::path::PathBuf;

use tokio::sync::mpsc;

/// Sent by a frontend to a session.
#[derive(Debug)]
pub enum Command {
    /// Run a turn for this user message. Sent while a turn runs, it joins
    /// that turn before its next model request, or starts the next turn if
    /// the running one makes no further request.
    Prompt { text: String },
    /// Stop the running turn; it ends with `StopReason::Cancelled`.
    /// Does nothing while idle.
    Cancel,
    /// Answer the `ApprovalRequested` event for the tool call with this `id`.
    Approval { id: u64, approve: bool },
}

/// Sent by a session to its frontend.
#[derive(Debug, PartialEq)]
pub enum Event {
    /// The model's context size in tokens, as the server reports it; None when it doesn't. Always the session's first event.
    ContextLimit { tokens: Option<u64> },
    /// A `Prompt`'s text entered the model's history: first in a turn it
    /// starts the turn, later it joins the running one.
    UserMessage { text: String },
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
    /// The turn waits `secs` seconds before its next model request.
    Paused { secs: u64 },
    /// The turn failed: its messages leave the history and `TurnEnded`
    /// follows. The session stays usable.
    Error { message: String },
    /// The running turn is over; a `Prompt` it didn't take starts the next.
    TurnEnded { stop: StopReason },
}

/// A change a tool call wants to make, shown to the user for approval.
#[derive(Debug, Clone, PartialEq)]
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
#[derive(Debug, Clone, PartialEq)]
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
#[derive(Debug, Clone, Copy, PartialEq)]
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
#[derive(Debug, Clone, Copy, PartialEq)]
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

/// What a frontend holds: a session's command sender and event receiver.
pub struct SessionHandle {
    pub commands: mpsc::Sender<Command>,
    pub events: mpsc::Receiver<Event>,
}

/// Why a `SessionFactory` could not create a session.
#[derive(Debug, thiserror::Error)]
pub enum SessionError {
    /// The session could not be started; the message says why.
    #[error("could not start a session: {0}")]
    Start(String),
}

/// Creates sessions. Frontends get one from the binary, so they never
/// depend on the core directly.
pub trait SessionFactory: Send + Sync {
    /// Start a session whose tools work inside `cwd`.
    fn new_session(&self, cwd: PathBuf) -> Result<SessionHandle, SessionError>;
}
