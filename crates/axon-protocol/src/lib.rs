//! Commands and events exchanged between the Axon core and its frontends.
//!
//! Everything here is frontend-agnostic: no model-provider, ACP or UI types.

use std::path::PathBuf;

use tokio::sync::mpsc;

/// Sent by a frontend to a session.
#[derive(Debug)]
pub enum Command {
    /// Run a turn for this user message.
    Prompt { text: String },
    /// Stop the running turn; it ends with `StopReason::Cancelled`.
    /// Does nothing while idle.
    Cancel,
}

/// Sent by a session to its frontend.
#[derive(Debug, PartialEq)]
pub enum Event {
    /// A piece of the model's reply, in order.
    TextDelta { text: String },
    /// The model called a tool; `args` is the JSON text it sent.
    ToolCallStarted { name: String, args: String },
    /// The tool call started last has finished.
    ToolCallFinished {
        name: String,
        status: ToolStatus,
        output: String,
    },
    /// Something went wrong; the session stays usable.
    Error { message: String },
    /// The turn started by the last `Prompt` is over.
    TurnEnded { stop: StopReason },
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ToolStatus {
    Completed,
    /// The tool reported an error; `output` says what went wrong.
    Failed,
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
