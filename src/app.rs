use tokio::sync::oneshot;

use crate::{client::AxonClient, config::Config};

#[derive(Debug)]
pub enum ChatMessage {
    User(String),
    Agent(String),
    ToolCall {
        name: String,
        args: serde_json::Value,
    },
    ToolResult {
        name: String,
        content: String,
    },
}

pub struct PendingDiff {
    pub path: String,
    pub before: String,
    pub after: String,
}

pub struct App {
    pub running: bool,
    pub config: Config,
    pub client: AxonClient,
    pub input: String,
    /// Kept for inference context (building OpenAI message history).
    pub messages: Vec<ChatMessage>,
    /// Agent tokens accumulating in the current streaming turn.
    pub streaming_text: String,
    /// Lines above the input row in the live area (streaming lines + separator).
    /// Used to jump back to the top of the live area on the next redraw.
    pub live_lines_to_top: u16,
    /// A diff waiting for user approval. While set, the live area shows the diff
    /// and an [y/n] prompt instead of the normal input.
    pub pending_diff: Option<PendingDiff>,
    /// Oneshot to signal the inference task when the user decides. `None` when
    /// the diff was sent in allow mode (auto-commit, no blocking).
    pub pending_approval: Option<oneshot::Sender<bool>>,
}

impl Default for App {
    fn default() -> Self {
        Self::new()
    }
}

impl App {
    pub fn new() -> Self {
        let config = Config::default();
        let client = AxonClient::new(&config.backend);
        Self {
            running: true,
            client,
            config,
            input: String::new(),
            messages: Vec::new(),
            streaming_text: String::new(),
            live_lines_to_top: 1,
            pending_diff: None,
            pending_approval: None,
        }
    }
}
