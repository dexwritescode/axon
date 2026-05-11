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

impl BackendConfig {
    /// Returns `Some` when `AXON_PLANNING_BASE_URL` is set, `None` otherwise.
    fn from_planning_env() -> Option<Self> {
        let base_url = std::env::var("AXON_PLANNING_BASE_URL").ok()?;
        Some(Self {
            base_url,
            api_key: std::env::var("AXON_PLANNING_API_KEY")
                .or_else(|_| std::env::var("OPENAI_API_KEY"))
                .unwrap_or_else(|_| "x".to_string()),
            model: std::env::var("AXON_PLANNING_MODEL").unwrap_or_default(),
        })
    }
}

pub struct Config {
    pub backend: BackendConfig,
    /// When `Some`, the planning phase (Harness Parser, Context Gatherer) uses this backend.
    /// When `None`, all inference goes to `backend` — single-model setups work out of the box.
    pub planning_backend: Option<BackendConfig>,
    pub tool_approval: ToolApproval,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            backend: BackendConfig::default(),
            planning_backend: BackendConfig::from_planning_env(),
            tool_approval: ToolApproval::Ask,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    // Env var tests mutate process-global state — serialize them.
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn planning_backend_none_when_env_unset() {
        let _guard = ENV_LOCK.lock().unwrap();
        unsafe { std::env::remove_var("AXON_PLANNING_BASE_URL") };
        assert!(Config::default().planning_backend.is_none());
    }

    #[test]
    fn planning_backend_some_when_env_set() {
        let _guard = ENV_LOCK.lock().unwrap();
        unsafe {
            std::env::set_var("AXON_PLANNING_BASE_URL", "http://localhost:9090/v1");
            std::env::set_var("AXON_PLANNING_MODEL", "llama-3-8b");
            std::env::remove_var("AXON_PLANNING_API_KEY");
        }
        let config = Config::default();
        unsafe {
            std::env::remove_var("AXON_PLANNING_BASE_URL");
            std::env::remove_var("AXON_PLANNING_MODEL");
        }
        let planning = config.planning_backend.expect("should be Some");
        assert_eq!(planning.base_url, "http://localhost:9090/v1");
        assert_eq!(planning.model, "llama-3-8b");
    }
}
