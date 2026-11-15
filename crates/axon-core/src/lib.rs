//! The Axon core: sessions that stream model replies to a frontend.
//! Knows nothing about any frontend.

mod model;
mod session;
#[cfg(test)]
mod test_support;

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
    use crate::test_support::{api_url, config, reply, respond};
    use axon_protocol::{Command, Event};
    use wiremock::MockServer;

    #[test]
    fn new_session_outside_a_runtime_is_an_error() {
        let result = CoreFactory::new(config("http://unused".into())).new_session();

        assert!(matches!(result, Err(SessionError::Start(_))));
    }

    #[tokio::test]
    async fn new_session_streams_a_reply_from_the_configured_model() {
        let server = MockServer::start().await;
        respond(&server, reply(&["Hello"]), 1).await;
        let mut handle = CoreFactory::new(config(api_url(&server)))
            .new_session()
            .unwrap();

        handle
            .commands
            .send(Command::Prompt { text: "hi".into() })
            .await
            .unwrap();

        assert_eq!(
            handle.events.recv().await,
            Some(Event::TextDelta {
                text: "Hello".into()
            })
        );
        assert_eq!(handle.events.recv().await, Some(Event::TurnEnded));
    }
}
