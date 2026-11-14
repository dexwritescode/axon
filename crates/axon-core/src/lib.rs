//! The Axon harness: session actor, agent loop, model client, tools and
//! approval policy. Knows nothing about any frontend.

mod model;
mod session;

use axon_protocol::{SessionError, SessionFactory, SessionHandle};
use model::ModelClient;

pub use model::ModelConfig;

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
    fn new_session(&self) -> Result<SessionHandle, SessionError> {
        tokio::runtime::Handle::try_current().map_err(|e| SessionError::Start(e.to_string()))?;
        Ok(session::spawn(ModelClient::new(&self.config)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axon_protocol::{Command, Event};
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn factory(base_url: String) -> CoreFactory {
        CoreFactory::new(ModelConfig {
            base_url,
            api_key: "test".into(),
            model: "test-model".into(),
        })
    }

    #[test]
    fn new_session_outside_a_runtime_is_an_error() {
        let result = factory("http://unused".into()).new_session();

        assert!(matches!(result, Err(SessionError::Start(_))));
    }

    #[tokio::test]
    async fn new_session_runs_a_turn_against_the_model() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/chat/completions"))
            .respond_with(ResponseTemplate::new(400))
            .expect(1)
            .mount(&server)
            .await;
        let mut handle = factory(format!("{}/v1", server.uri()))
            .new_session()
            .unwrap();

        handle
            .commands
            .send(Command::Prompt { text: "hi".into() })
            .await
            .unwrap();

        assert!(matches!(
            handle.events.recv().await,
            Some(Event::Error { .. })
        ));
        assert_eq!(handle.events.recv().await, Some(Event::TurnEnded));
    }
}
