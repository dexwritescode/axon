//! Streaming client for OpenAI-compatible chat completion servers.

use std::collections::BTreeMap;
use std::fmt;
use std::time::Duration;

use anyhow::Result;
use async_openai::{
    Client,
    config::OpenAIConfig,
    error::OpenAIError,
    types::chat::{
        ChatCompletionMessageToolCallChunk, ChatCompletionRequestMessage, ChatCompletionTools,
        CreateChatCompletionRequestArgs,
    },
};
use futures::{Stream, StreamExt, stream};

/// Where to find the model and which one to use.
pub struct ModelConfig {
    pub base_url: String,
    pub api_key: String,
    pub model: String,
    /// Wait between a turn's model requests, to let the machine cool.
    pub request_pause: Duration,
}

impl fmt::Debug for ModelConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ModelConfig")
            .field("base_url", &self.base_url)
            .field("api_key", &"<redacted>")
            .field("model", &self.model)
            .field("request_pause", &self.request_pause)
            .finish()
    }
}

/// A piece of a streamed reply.
#[derive(Debug)]
pub(crate) enum Delta {
    Text(String),
    /// A fragment of a tool call; `ToolCalls` puts them back together.
    ToolCall(ChatCompletionMessageToolCallChunk),
}

/// A complete tool call. `args` is the JSON text as the model sent it.
#[derive(Debug, Default, PartialEq)]
pub(crate) struct ToolCall {
    pub(crate) id: String,
    pub(crate) name: String,
    pub(crate) args: String,
}

/// Assembles tool calls from their streamed fragments.
#[derive(Default)]
pub(crate) struct ToolCalls(BTreeMap<u32, ToolCall>);

impl ToolCalls {
    pub(crate) fn push(&mut self, chunk: ChatCompletionMessageToolCallChunk) {
        let call = self.0.entry(chunk.index).or_default();
        if let Some(id) = chunk.id {
            call.id = id;
        }
        if let Some(function) = chunk.function {
            if let Some(name) = function.name {
                call.name = name;
            }
            if let Some(args) = function.arguments {
                call.args.push_str(&args);
            }
        }
    }

    /// The calls in the order the model made them.
    pub(crate) fn finish(self) -> Vec<ToolCall> {
        self.0.into_values().collect()
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

    /// Stream the model's reply: text deltas and tool-call fragments, in order.
    /// Dropping the stream cancels the request.
    pub(crate) async fn stream_chat(
        &self,
        messages: Vec<ChatCompletionRequestMessage>,
        tools: Vec<ChatCompletionTools>,
    ) -> Result<impl Stream<Item = Result<Delta>> + use<>> {
        let request = CreateChatCompletionRequestArgs::default()
            .model(&self.model)
            .messages(messages)
            .tools(tools)
            .stream(true)
            .build()?;
        let stream = self.inner.chat().create_stream(request).await?;

        Ok(stream.flat_map(|chunk| {
            let deltas: Vec<Result<Delta>> = match chunk {
                Err(e) => vec![Err(server_error(e))],
                Ok(chunk) => chunk
                    .choices
                    .into_iter()
                    .flat_map(|choice| {
                        let text = choice.delta.content.filter(|text| !text.is_empty());
                        let calls = choice.delta.tool_calls.unwrap_or_default();
                        text.map(Delta::Text)
                            .into_iter()
                            .chain(calls.into_iter().map(Delta::ToolCall))
                    })
                    .map(Ok)
                    .collect(),
            };
            stream::iter(deltas)
        }))
    }
}

/// A server that fails mid-stream sends an error object where a chunk should
/// be; report its message rather than the failed chunk parse.
fn server_error(error: OpenAIError) -> anyhow::Error {
    if let OpenAIError::JSONDeserialize(_, body) = &error
        && let Ok(json) = serde_json::from_str::<serde_json::Value>(body)
        && let Some(message) = json["error"]["message"].as_str().or(json["error"].as_str())
    {
        return anyhow::anyhow!("server error: {message}");
    }
    error.into()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{api_url, chunk, client_error, config, respond, sse};
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
        respond(&server, response).await;
        let client = ModelClient::new(&config(api_url(&server)));
        (server, client)
    }

    /// Run a request to the end: its text deltas and assembled tool calls,
    /// or the first error.
    async fn collect(client: &ModelClient) -> Result<(Vec<String>, Vec<ToolCall>)> {
        let stream = client.stream_chat(vec![user("hi")], Vec::new()).await?;
        let mut texts = Vec::new();
        let mut calls = ToolCalls::default();
        for delta in stream.collect::<Vec<_>>().await {
            match delta? {
                Delta::Text(text) => texts.push(text),
                Delta::ToolCall(chunk) => calls.push(chunk),
            }
        }
        Ok((texts, calls.finish()))
    }

    fn call(id: &str, name: &str, args: &str) -> ToolCall {
        ToolCall {
            id: id.into(),
            name: name.into(),
            args: args.into(),
        }
    }

    #[tokio::test]
    async fn an_error_object_in_the_stream_reports_the_servers_message() {
        let error =
            json!({"error": {"message": "The model has crashed without additional information."}});
        let (_server, client) = client_for(sse(&[error.to_string()])).await;

        let message = collect(&client).await.unwrap_err().to_string();

        assert_eq!(
            message,
            "server error: The model has crashed without additional information."
        );
    }

    #[tokio::test]
    async fn an_error_string_in_the_stream_reports_it() {
        let (_server, client) =
            client_for(sse(&[json!({"error": "model unloaded"}).to_string()])).await;

        let message = collect(&client).await.unwrap_err().to_string();

        assert_eq!(message, "server error: model unloaded");
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

        let (texts, calls) = collect(&client).await.unwrap();
        assert_eq!(texts, ["Hello", ", ", "world"]);
        assert!(calls.is_empty());
    }

    #[tokio::test]
    async fn tool_call_fragments_are_assembled_in_index_order() {
        let fragment = |index: u32, rest: serde_json::Value| {
            let mut call = json!({"index": index});
            call.as_object_mut()
                .unwrap()
                .extend(rest.as_object().unwrap().clone());
            chunk(json!({"tool_calls": [call]}), None)
        };
        let (_server, client) = client_for(sse(&[
            chunk(json!({"content": "Looking"}), None),
            fragment(1, json!({"id": "b", "type": "function", "function": {"name": "list_dir", "arguments": ""}})),
            fragment(0, json!({"id": "a", "type": "function", "function": {"name": "read_file", "arguments": "{\"pa"}})),
            fragment(1, json!({"function": {"arguments": "{}"}})),
            fragment(0, json!({"function": {"arguments": "th\":\"x\"}"}})),
            chunk(json!({}), Some("tool_calls")),
        ]))
        .await;

        let (texts, calls) = collect(&client).await.unwrap();
        assert_eq!(texts, ["Looking"]);
        assert_eq!(
            calls,
            [
                call("a", "read_file", r#"{"path":"x"}"#),
                call("b", "list_dir", "{}")
            ]
        );
    }

    #[tokio::test]
    async fn http_error_is_returned_as_an_error() {
        let (_server, client) = client_for(client_error()).await;

        assert!(collect(&client).await.is_err());
    }
}
