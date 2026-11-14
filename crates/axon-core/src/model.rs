//! Streaming client for OpenAI-compatible chat completion servers.

use anyhow::Result;
use async_openai::{
    Client,
    config::OpenAIConfig,
    types::chat::{ChatCompletionRequestMessage, CreateChatCompletionRequestArgs},
};
use futures::{Stream, StreamExt};

/// Where to find the model and which one to use.
#[derive(Debug, Clone)]
pub struct ModelConfig {
    pub base_url: String,
    pub api_key: String,
    pub model: String,
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
    use async_openai::types::chat::{
        ChatCompletionRequestUserMessage, ChatCompletionRequestUserMessageContent,
    };
    use serde_json::json;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn user(text: &str) -> ChatCompletionRequestMessage {
        ChatCompletionRequestMessage::User(ChatCompletionRequestUserMessage {
            content: ChatCompletionRequestUserMessageContent::Text(text.into()),
            name: None,
        })
    }

    fn chunk(delta: serde_json::Value, finish_reason: Option<&str>) -> String {
        json!({
            "id": "chatcmpl-test",
            "object": "chat.completion.chunk",
            "created": 0,
            "model": "test-model",
            "choices": [{"index": 0, "delta": delta, "finish_reason": finish_reason}]
        })
        .to_string()
    }

    fn sse(chunks: &[String]) -> String {
        let mut body: String = chunks.iter().map(|c| format!("data: {c}\n\n")).collect();
        body.push_str("data: [DONE]\n\n");
        body
    }

    async fn client_for(response: ResponseTemplate) -> (MockServer, ModelClient) {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/chat/completions"))
            .respond_with(response)
            .mount(&server)
            .await;
        let client = ModelClient::new(&ModelConfig {
            base_url: format!("{}/v1", server.uri()),
            api_key: "test".into(),
            model: "test-model".into(),
        });
        (server, client)
    }

    /// Run a request to the end, collecting the text deltas or the first error.
    async fn collect(client: &ModelClient) -> Result<Vec<String>> {
        let stream = client.stream_text(vec![user("hi")]).await?;
        stream.collect::<Vec<_>>().await.into_iter().collect()
    }

    #[tokio::test]
    async fn streams_text_deltas_in_order_and_stops_at_done() {
        let body = sse(&[
            chunk(json!({"role": "assistant", "content": "Hello"}), None),
            chunk(json!({"content": ", "}), None),
            chunk(json!({"content": "world"}), None),
            chunk(json!({}), Some("stop")),
        ]);
        let (_server, client) = client_for(
            ResponseTemplate::new(200)
                .insert_header("content-type", "text/event-stream")
                .set_body_string(body),
        )
        .await;

        assert_eq!(collect(&client).await.unwrap(), ["Hello", ", ", "world"]);
    }

    #[tokio::test]
    async fn http_error_is_returned_as_an_error() {
        let (_server, client) = client_for(ResponseTemplate::new(500)).await;

        assert!(collect(&client).await.is_err());
    }
}
