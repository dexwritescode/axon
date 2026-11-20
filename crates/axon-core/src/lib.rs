//! The Axon core: sessions that run an agent loop with file tools (reads,
//! and writes the user approves) and stream it to a frontend. Knows
//! nothing about any frontend.

mod backend;
mod context;
mod jail;
#[cfg(test)]
mod live;
mod mcp;
mod model;
mod prompt;
mod session;
mod store;
#[cfg(test)]
mod test_support;
mod tools;
mod turn;

use std::path::PathBuf;
use std::time::Duration;

use axon_protocol::{McpServer, Resume, SessionError, SessionFactory, SessionHandle};
use model::ModelClient;
use store::Store;
use turn::Agent;

pub use backend::{Backend, ReasoningEffort, UnknownBackend, UnknownReasoningEffort};
pub use model::{ModelConfig, RequestPause};

/// Model requests per prompt before a turn stops.
const MAX_TURN_REQUESTS: usize = 25;

/// How long context discovery waits for the server before the session starts without it.
const DISCOVERY_TIMEOUT: Duration = Duration::from_secs(2);

/// What a profile changes about the agent; the default keeps Axon's own.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct AgentConfig {
    /// Replaces Axon's system prompt.
    pub system_prompt: Option<String>,
    /// The built-in tools offered to the model, by name; None offers all.
    pub tools: Option<Vec<String>>,
    /// MCP tools that run without asking, by name; every other MCP tool asks.
    pub allow: Vec<String>,
}

/// The names of the built-in tools.
pub fn tool_names() -> Vec<String> {
    tools::schemas()
        .iter()
        .map(|tool| tools::name(tool).to_string())
        .collect()
}

/// Starts core sessions that all talk to the same model.
pub struct CoreFactory {
    config: ModelConfig,
    agent: AgentConfig,
    /// Sessions are saved under here, a folder per working directory.
    sessions: PathBuf,
}

impl CoreFactory {
    pub fn new(config: ModelConfig, agent: AgentConfig, sessions: PathBuf) -> Self {
        Self {
            config,
            agent,
            sessions,
        }
    }
}

impl SessionFactory for CoreFactory {
    /// Must be called inside a tokio runtime; the session runs as a task on it.
    fn new_session(
        &self,
        cwd: PathBuf,
        resume: Resume,
        mcp_servers: Vec<McpServer>,
    ) -> Result<SessionHandle, SessionError> {
        tokio::runtime::Handle::try_current().map_err(|e| SessionError::Start(e.to_string()))?;
        let http = reqwest::Client::builder()
            .timeout(DISCOVERY_TIMEOUT)
            .build()
            .map_err(|e| SessionError::Start(e.to_string()))?;
        let cwd = cwd
            .canonicalize()
            .map_err(|e| SessionError::Start(format!("{}: {e}", cwd.display())))?;
        let (store, saved) = Store::open(&self.sessions, &cwd, &self.config.model, &resume)
            .map_err(|e| SessionError::Start(format!("{e:#}")))?
            .ok_or(SessionError::NotFound)?;
        let backend = self.config.backend;
        let base_url = self.config.base_url.clone();
        let api_key = self.config.api_key.clone();
        let model = self.config.model.clone();
        let system = self
            .agent
            .system_prompt
            .clone()
            .unwrap_or_else(|| prompt::system(&cwd));
        let tools = tools::enabled(self.agent.tools.as_deref());
        let taken = tools
            .iter()
            .map(|tool| tools::name(tool).to_string())
            .collect();
        let mcp = mcp::start(mcp_servers, cwd.clone(), taken, self.agent.allow.clone());
        Ok(session::spawn(
            Agent {
                client: ModelClient::new(&self.config),
                system,
                tools,
                mcp: Default::default(),
                cwd,
                max_requests: MAX_TURN_REQUESTS,
                request_pause: self.config.request_pause,
                seen: Default::default(),
            },
            async move { context::discover(&http, backend, &base_url, &api_key, &model).await },
            mcp,
            store,
            saved,
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{api_url, config, delta, end, prompt, reply, respond};
    use axon_protocol::{Event, StopReason};
    use wiremock::MockServer;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, ResponseTemplate};

    /// A factory saving sessions in a temporary folder, which it returns too.
    fn factory(base_url: String) -> (CoreFactory, tempfile::TempDir) {
        factory_with(base_url, AgentConfig::default())
    }

    /// Like `factory`, with what a profile set.
    fn factory_with(base_url: String, agent: AgentConfig) -> (CoreFactory, tempfile::TempDir) {
        let root = tempfile::tempdir().unwrap();
        (
            CoreFactory::new(config(base_url), agent, root.path().into()),
            root,
        )
    }

    /// The body of the first chat request `server` received, after context discovery.
    async fn first_request(server: &MockServer) -> serde_json::Value {
        let requests = server.received_requests().await.unwrap();
        let chat = requests
            .iter()
            .find(|request| request.url.path() == "/v1/chat/completions")
            .expect("no chat request");
        chat.body_json().unwrap()
    }

    #[test]
    fn new_session_outside_a_runtime_is_an_error() {
        let (factory, _root) = factory("http://unused".into());
        let result = factory.new_session(".".into(), Resume::New, vec![]);

        assert!(matches!(result, Err(SessionError::Start(_))));
    }

    #[tokio::test]
    async fn new_session_in_a_missing_directory_is_an_error() {
        let (factory, _root) = factory("http://unused".into());
        let result = factory.new_session("/no/such/dir".into(), Resume::New, vec![]);

        assert!(matches!(result, Err(SessionError::Start(e)) if e.starts_with("/no/such/dir")));
    }

    #[tokio::test]
    async fn resuming_with_nothing_saved_is_not_found() {
        let (factory, _root) = factory("http://unused".into());

        for resume in [Resume::Latest, Resume::Id("0193".into())] {
            let result = factory.new_session(".".into(), resume, vec![]);
            assert!(matches!(result, Err(SessionError::NotFound)));
        }
    }

    #[tokio::test]
    async fn new_session_streams_a_reply_from_the_configured_model() {
        let server = MockServer::start().await;
        respond(&server, reply(&["Hello"])).await;
        let (factory, _root) = factory(api_url(&server));
        let mut handle = factory
            .new_session(".".into(), Resume::New, vec![])
            .unwrap();

        assert_eq!(
            handle.events.recv().await,
            Some(Event::ContextLimit { tokens: None })
        );
        assert_eq!(
            prompt(&mut handle, "hi").await,
            [delta("Hello"), end(StopReason::EndTurn)]
        );
    }

    #[tokio::test]
    async fn a_profile_sets_the_system_prompt_and_tools_the_model_gets() {
        let server = MockServer::start().await;
        respond(&server, reply(&["Hi"])).await;
        let agent = AgentConfig {
            system_prompt: Some("You are BUDDY.".into()),
            tools: Some(vec!["grep".into(), "read_file".into()]),
            ..AgentConfig::default()
        };
        let (factory, _root) = factory_with(api_url(&server), agent);
        let mut handle = factory
            .new_session(".".into(), Resume::New, vec![])
            .unwrap();
        handle.events.recv().await;

        prompt(&mut handle, "hi").await;

        let body = first_request(&server).await;
        assert_eq!(body["messages"][0]["content"], "You are BUDDY.");
        let names: Vec<&str> = body["tools"]
            .as_array()
            .unwrap()
            .iter()
            .map(|tool| tool["function"]["name"].as_str().unwrap())
            .collect();
        assert_eq!(names, ["read_file", "grep"]);
    }

    #[tokio::test]
    async fn with_every_tool_off_the_request_sends_no_tools() {
        let server = MockServer::start().await;
        respond(&server, reply(&["Hi"])).await;
        let agent = AgentConfig {
            tools: Some(vec![]),
            ..AgentConfig::default()
        };
        let (factory, _root) = factory_with(api_url(&server), agent);
        let mut handle = factory
            .new_session(".".into(), Resume::New, vec![])
            .unwrap();
        handle.events.recv().await;

        prompt(&mut handle, "hi").await;

        let body = first_request(&server).await;
        assert!(body.get("tools").is_none(), "{body}");
        let system = body["messages"][0]["content"].as_str().unwrap();
        assert!(system.starts_with("You are Axon"), "{system}");
    }

    #[tokio::test]
    async fn new_session_first_reports_the_servers_context_limit() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/v0/models/test-model"))
            .respond_with(
                ResponseTemplate::new(200).set_body_string(r#"{"loaded_context_length": 128000}"#),
            )
            .mount(&server)
            .await;
        let (factory, _root) = factory(api_url(&server));
        let mut handle = factory
            .new_session(".".into(), Resume::New, vec![])
            .unwrap();

        assert_eq!(
            handle.events.recv().await,
            Some(Event::ContextLimit {
                tokens: Some(128000)
            })
        );
    }
}
