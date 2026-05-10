#[allow(dead_code)]
#[derive(Clone)]
pub enum ToolApproval {
    Allow,
    Deny,
    Ask,
}

impl ToolApproval {
    pub fn as_str(&self) -> &'static str {
        match self {
            ToolApproval::Allow => "allow",
            ToolApproval::Deny => "deny",
            ToolApproval::Ask => "ask",
        }
    }
}

pub struct BackendConfig {
    pub base_url: String,
    #[allow(dead_code)]
    pub api_key: String,
    pub model: String,
}

impl Default for BackendConfig {
    fn default() -> Self {
        Self {
            base_url: std::env::var("OPENAI_BASE_URL")
                .unwrap_or_else(|_| "http://localhost:8080/v1".to_string()),
            api_key: std::env::var("OPENAI_API_KEY").unwrap_or_else(|_| "x".to_string()),
            model: std::env::var("AXON_MODEL").unwrap_or_default(),
        }
    }
}

pub struct Config {
    pub backend: BackendConfig,
    pub tool_approval: ToolApproval,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            backend: BackendConfig::default(),
            tool_approval: ToolApproval::Ask,
        }
    }
}
