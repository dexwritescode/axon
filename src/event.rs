use tokio::sync::oneshot;

#[allow(dead_code)]
#[derive(Debug)]
pub enum AppEvent {
    Token(String),
    ToolCall {
        id: String,
        name: String,
        args: serde_json::Value,
    },
    ToolResult {
        name: String,
        content: String,
    },
    FileDiff {
        path: String,
        before: String,
        after: String,
        /// Present in `ask` mode — inference blocks on this until the user decides.
        /// `None` in `allow` mode (diff shown but auto-committed).
        approval: Option<oneshot::Sender<bool>>,
    },
    Done,
}

impl PartialEq for AppEvent {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Token(a), Self::Token(b)) => a == b,
            (
                Self::ToolCall { id: a, name: b, args: c },
                Self::ToolCall { id: d, name: e, args: f },
            ) => a == d && b == e && c == f,
            (
                Self::ToolResult { name: a, content: b },
                Self::ToolResult { name: c, content: d },
            ) => a == c && b == d,
            (Self::Done, Self::Done) => true,
            _ => false,
        }
    }
}
