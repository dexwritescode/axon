//! Commands and events exchanged between the Axon core and its frontends.
//!
//! Everything here is frontend-agnostic: no model-provider, ACP or UI types.

use tokio::sync::mpsc;

/// Sent by a frontend to a session.
#[derive(Debug)]
pub enum Command {
    /// Run a turn for this user message.
    Prompt { text: String },
}

/// Sent by a session to its frontend.
#[derive(Debug, PartialEq)]
pub enum Event {
    /// A piece of the model's reply, in order.
    TextDelta { text: String },
    /// Something went wrong; the session stays usable.
    Error { message: String },
    /// The turn started by the last `Prompt` is over.
    TurnEnded,
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
    fn new_session(&self) -> Result<SessionHandle, SessionError>;
}
