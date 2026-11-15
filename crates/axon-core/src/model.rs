//! Streaming client for OpenAI-compatible chat completion servers.

use std::fmt;

use anyhow::Result;
use async_openai::{
    Client,
    config::OpenAIConfig,
    types::chat::{ChatCompletionRequestMessage, CreateChatCompletionRequestArgs},
};
use futures::{Stream, StreamExt};

/// Where to find the model and which one to use.
pub struct ModelConfig {
    pub base_url: String,
    pub api_key: String,
    pub model: String,
}

impl fmt::Debug for ModelConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ModelConfig")
            .field("base_url", &self.base_url)
            .field("api_key", &"<redacted>")
            .field("model", &self.model)
            .finish()
    }
}

pub(crate) struct ModelClient {
    inner: Client<OpenAIConfig>,
    model: String,
}

impl ModelClient {
    pub(crate) fn new(config: &ModelConfig) -> Self {
        let openai = OpenAIConfig::new()
            .with_api_base(&config.base_url)
            .with_api_key(&config.api_key);
        Self {
            inner: Client::with_config(openai),
            model: config.model.clone(),
        }
    }

    /// Stream the text of the model's reply, one delta at a time.
    /// Dropping the stream cancels the request.
    pub(crate) async fn stream_text(
        &self,
        messages: Vec<ChatCompletionRequestMessage>,
    ) -> Result<impl Stream<Item = Result<String>> + use<>> {
        let request = CreateChatCompletionRequestArgs::default()
            .model(&self.model)
            .messages(messages)
            .stream(true)
            .build()?;
        let stream = self.inner.chat().create_stream(request).await?;

        Ok(stream.filter_map(|chunk| async move {
            match chunk {
                Err(e) => Some(Err(e.into())),
                Ok(chunk) => {
                    let text: String = chunk
                        .choices
                        .into_iter()
                        .filter_map(|choice| choice.delta.content)
                        .collect();
                    (!text.is_empty()).then_some(Ok(text))
                }
            }
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{api_url, chunk, config, respond, sse};
    use async_openai::types::chat::{
        ChatCompletionRequestUserMessage, ChatCompletionRequestUserMessageContent,
    };
    use serde_json::json;
    use wiremock::{MockServer, ResponseTemplate};

    fn user(text: &str) -> ChatCompletionRequestMessage {
        ChatCompletionRequestMessage::User(ChatCompletionRequestUserMessage {
            content: ChatCompletionRequestUserMessageContent::Text(text.into()),
            name: None,
        })
    }

    async fn client_for(response: ResponseTemplate) -> (MockServer, ModelClient) {
        let server = MockServer::start().await;
        respond(&server, response, 1).await;
        let client = ModelClient::new(&config(api_url(&server)));
        (server, client)
    }

    /// Run a request to the end, collecting the text deltas or the first error.
    async fn collect(client: &ModelClient) -> Result<Vec<String>> {
        let stream = client.stream_text(vec![user("hi")]).await?;
        stream.collect::<Vec<_>>().await.into_iter().collect()
    }

    #[test]
    fn debug_hides_the_api_key() {
        let mut config = config("http://unused".into());
        config.api_key = "sk-secret".into();

        assert!(!format!("{config:?}").contains("sk-secret"));
    }

    #[tokio::test]
    async fn streams_text_deltas_in_order_skipping_empty_ones() {
        let (_server, client) = client_for(sse(&[
            chunk(json!({"role": "assistant", "content": "Hello"}), None),
            chunk(json!({"content": ", "}), None),
            chunk(json!({"content": "world"}), None),
            chunk(json!({}), Some("stop")),
        ]))
        .await;

        assert_eq!(collect(&client).await.unwrap(), ["Hello", ", ", "world"]);
    }

    #[tokio::test]
    async fn http_error_is_returned_as_an_error() {
        // Not 5xx: async-openai's default retry layer retries those with backoff.
        let (_server, client) = client_for(ResponseTemplate::new(400)).await;

        assert!(collect(&client).await.is_err());
    }
}
