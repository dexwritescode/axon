//! The Axon core: sessions that run an agent loop with file tools (reads,
//! and writes the user approves) and stream it to a frontend. Knows
//! nothing about any frontend.

mod backend;
mod context;
mod jail;
#[cfg(test)]
mod live;
mod model;
mod prompt;
mod session;
#[cfg(test)]
mod test_support;
mod tools;
mod turn;

use std::path::PathBuf;
use std::time::Duration;

use axon_protocol::{SessionError, SessionFactory, SessionHandle};
use model::ModelClient;
use turn::Agent;

pub use backend::{Backend, UnknownBackend};
pub use model::ModelConfig;

/// Model requests per prompt before a turn stops.
const MAX_TURN_REQUESTS: usize = 25;

/// How long context discovery waits for the server before the session starts without it.
const DISCOVERY_TIMEOUT: Duration = Duration::from_secs(2);

/// Starts core sessions that all talk to the same model.
pub struct CoreFactory {
    config: ModelConfig,
}

impl CoreFactory {
    pub fn new(config: ModelConfig) -> Self {
        Self { config }
    }
}

impl SessionFactory for CoreFactory {
    /// Must be called inside a tokio runtime; the session runs as a task on it.
    fn new_session(&self, cwd: PathBuf) -> Result<SessionHandle, SessionError> {
        tokio::runtime::Handle::try_current().map_err(|e| SessionError::Start(e.to_string()))?;
        let http = reqwest::Client::builder()
            .timeout(DISCOVERY_TIMEOUT)
            .build()
            .map_err(|e| SessionError::Start(e.to_string()))?;
        let cwd = cwd
            .canonicalize()
            .map_err(|e| SessionError::Start(format!("{}: {e}", cwd.display())))?;
        let backend = self.config.backend;
        let base_url = self.config.base_url.clone();
        let api_key = self.config.api_key.clone();
        let model = self.config.model.clone();
        Ok(session::spawn(
            Agent {
                client: ModelClient::new(&self.config),
                cwd,
                max_requests: MAX_TURN_REQUESTS,
                request_pause: self.config.request_pause,
                seen: Default::default(),
            },
            async move { context::discover(&http, backend, &base_url, &api_key, &model).await },
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

    fn factory(base_url: String) -> CoreFactory {
        CoreFactory::new(config(base_url))
    }

    #[test]
    fn new_session_outside_a_runtime_is_an_error() {
        let result = factory("http://unused".into()).new_session(".".into());

        assert!(matches!(result, Err(SessionError::Start(_))));
    }

    #[tokio::test]
    async fn new_session_in_a_missing_directory_is_an_error() {
        let result = factory("http://unused".into()).new_session("/no/such/dir".into());

        assert!(matches!(result, Err(SessionError::Start(e)) if e.starts_with("/no/such/dir")));
    }

    #[tokio::test]
    async fn new_session_streams_a_reply_from_the_configured_model() {
        let server = MockServer::start().await;
        respond(&server, reply(&["Hello"])).await;
        let mut handle = factory(api_url(&server)).new_session(".".into()).unwrap();

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
    async fn new_session_first_reports_the_servers_context_limit() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/v0/models/test-model"))
            .respond_with(
                ResponseTemplate::new(200).set_body_string(r#"{"loaded_context_length": 128000}"#),
            )
            .mount(&server)
            .await;
        let mut handle = factory(api_url(&server)).new_session(".".into()).unwrap();

        assert_eq!(
            handle.events.recv().await,
            Some(Event::ContextLimit {
                tokens: Some(128000)
            })
        );
    }
}
