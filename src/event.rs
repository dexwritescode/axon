#[allow(dead_code)]
#[derive(Debug, PartialEq)]
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
