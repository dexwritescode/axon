//! The Axon core: sessions that run an agent loop with file tools (reads,
//! and writes the user approves) and stream it to a frontend. Knows
//! nothing about any frontend.

mod jail;
mod model;
mod prompt;
mod session;
#[cfg(test)]
mod test_support;
mod tools;
mod turn;

use std::path::PathBuf;

use axon_protocol::{SessionError, SessionFactory, SessionHandle};
use model::ModelClient;
use turn::Agent;

pub use model::ModelConfig;

/// Model requests per prompt before a turn stops.
const MAX_TURN_REQUESTS: usize = 25;

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
        let cwd = cwd
            .canonicalize()
            .map_err(|e| SessionError::Start(format!("{}: {e}", cwd.display())))?;
        Ok(session::spawn(Agent {
            client: ModelClient::new(&self.config),
            cwd,
            max_requests: MAX_TURN_REQUESTS,
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{api_url, config, prompt, reply, respond};
    use axon_protocol::{Event, StopReason};
    use wiremock::MockServer;

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
            prompt(&mut handle, "hi").await,
            [
                Event::TextDelta {
                    text: "Hello".into()
                },
                Event::TurnEnded {
                    stop: StopReason::EndTurn
                }
            ]
        );
    }
}
