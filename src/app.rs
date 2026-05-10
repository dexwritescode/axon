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

pub struct App {
    pub running: bool,
    pub config: Config,
    pub client: AxonClient,
    pub input: String,
    pub messages: Vec<ChatMessage>,
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
        }
    }
}
