#[allow(dead_code)]
pub enum AppEvent {
    Token(String),
    ToolCall {
        name: String,
        args: serde_json::Value,
    },
    ToolResult {
        name: String,
        content: String,
    },
    Done,
}
