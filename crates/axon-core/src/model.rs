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
        CreateChatCompletionRequestArgs, ReasoningEffort as Effort,
    },
};
use futures::{Stream, StreamExt, stream};
use serde::Deserialize;

use crate::backend::{Backend, ReasoningEffort};

/// How long to wait before each of a turn's model requests after the first.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum RequestPause {
    /// The same wait every time.
    Fixed(Duration),
    /// `ratio` times as long as the request before took, so a long request
    /// gets a long rest; at most `max` when set.
    Ratio { ratio: f64, max: Option<Duration> },
}

impl RequestPause {
    /// The wait after a request that took `took`.
    pub(crate) fn after(self, took: Duration) -> Duration {
        match self {
            Self::Fixed(pause) => pause,
            Self::Ratio { ratio, max } => {
                // Saturates instead of panicking when the product overflows.
                let pause = Duration::try_from_secs_f64(took.as_secs_f64() * ratio)
                    .unwrap_or(Duration::MAX);
                max.map_or(pause, |max| pause.min(max))
            }
        }
    }
}

/// Where to find the model and which one to use.
pub struct ModelConfig {
    pub backend: Backend,
    pub base_url: String,
    pub api_key: String,
    pub model: String,
    /// Wait between a turn's model requests, to let the machine cool.
    pub request_pause: RequestPause,
    /// Most tokens one model reply may take, reasoning included.
    pub max_tokens: u32,
    /// Sent with each request when set; otherwise the server decides.
    pub reasoning_effort: Option<ReasoningEffort>,
}

impl fmt::Debug for ModelConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ModelConfig")
            .field("backend", &self.backend)
            .field("base_url", &self.base_url)
            .field("api_key", &"<redacted>")
            .field("model", &self.model)
            .field("request_pause", &self.request_pause)
            .field("max_tokens", &self.max_tokens)
            .field("reasoning_effort", &self.reasoning_effort)
            .finish()
    }
}

/// A piece of a streamed reply.
#[derive(Debug)]
pub(crate) enum Delta {
    Text(String),
    /// A piece of the model's reasoning.
    Reasoning(String),
    /// A fragment of a tool call; `ToolCalls` puts them back together.
    ToolCall(ChatCompletionMessageToolCallChunk),
    /// The reply hit `max_tokens` and was cut off.
    Truncated,
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
    /// Add a fragment; returns the call it belongs to, as its name and the
    /// bytes of arguments so far, once the name is known.
    pub(crate) fn push(
        &mut self,
        chunk: ChatCompletionMessageToolCallChunk,
    ) -> Option<(&str, usize)> {
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
        (!call.name.is_empty()).then_some((call.name.as_str(), call.args.len()))
    }

    /// The calls in the order the model made them.
    pub(crate) fn finish(self) -> Vec<ToolCall> {
        self.0.into_values().collect()
    }
}

pub(crate) struct ModelClient {
    inner: Client<OpenAIConfig>,
    model: String,
    backend: Backend,
    max_tokens: u32,
    reasoning_effort: Option<ReasoningEffort>,
}

impl ModelClient {
    pub(crate) fn new(config: &ModelConfig) -> Self {
        let openai = OpenAIConfig::new()
            .with_api_base(&config.base_url)
            .with_api_key(&config.api_key);
        Self {
            inner: Client::with_config(openai),
            model: config.model.clone(),
            backend: config.backend,
            max_tokens: config.max_tokens,
            reasoning_effort: config.reasoning_effort,
        }
    }

    /// Stream the model's reply: reasoning, text and tool-call fragments, in order.
    /// Dropping the stream cancels the request.
    pub(crate) async fn stream_chat(
        &self,
        messages: Vec<ChatCompletionRequestMessage>,
        tools: Vec<ChatCompletionTools>,
    ) -> Result<impl Stream<Item = Result<Delta>> + use<>> {
        let mut request = CreateChatCompletionRequestArgs::default();
        request.model(&self.model).messages(messages).stream(true);
        // Some servers reject an empty list; with no tools, send none.
        if !tools.is_empty() {
            request.tools(tools);
        }
        match self.backend {
            Backend::Ollama => request.max_tokens(self.max_tokens),
            _ => request.max_completion_tokens(self.max_tokens),
        };
        if let Some(effort) = self.reasoning_effort {
            request.reasoning_effort(match effort {
                ReasoningEffort::None => Effort::None,
                ReasoningEffort::Low => Effort::Low,
                ReasoningEffort::Medium => Effort::Medium,
                ReasoningEffort::High => Effort::High,
            });
        }
        let request = request.build()?;
        let stream = self
            .inner
            .chat()
            .create_stream_byot::<_, Chunk>(request)
            .await?;

        Ok(stream.flat_map(|chunk| {
            let deltas: Vec<Result<Delta>> = match chunk {
                Err(e) => vec![Err(server_error(e))],
                Ok(chunk) => chunk
                    .choices
                    .into_iter()
                    .flat_map(|choice| {
                        let reasoning = choice
                            .delta
                            .reasoning
                            .or(choice.delta.reasoning_content)
                            .filter(|text| !text.is_empty());
                        let text = choice.delta.content.filter(|text| !text.is_empty());
                        let calls = choice.delta.tool_calls.unwrap_or_default();
                        let truncated = choice.finish_reason.as_deref() == Some("length");
                        reasoning
                            .map(Delta::Reasoning)
                            .into_iter()
                            .chain(text.map(Delta::Text))
                            .chain(calls.into_iter().map(Delta::ToolCall))
                            .chain(truncated.then_some(Delta::Truncated))
                    })
                    .map(Ok)
                    .collect(),
            };
            stream::iter(deltas)
        }))
    }
}

/// A streamed chunk, read with the reasoning that async-openai's own type drops.
/// `choices` is required, so an error object in its place fails to parse.
#[derive(Deserialize)]
struct Chunk {
    choices: Vec<Choice>,
}

#[derive(Deserialize)]
struct Choice {
    delta: ChunkDelta,
    /// A string, so a server's own reason (`abort`, `eos`) still parses.
    finish_reason: Option<String>,
}

/// Servers name the reasoning `reasoning_content` (LM Studio, llama.cpp) or
/// `reasoning` (Ollama), and some send both.
#[derive(Deserialize)]
struct ChunkDelta {
    content: Option<String>,
    reasoning: Option<String>,
    reasoning_content: Option<String>,
    tool_calls: Option<Vec<ChatCompletionMessageToolCallChunk>>,
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
    use crate::test_support::{api_url, chunk, client_error, config, reply, respond, sse};
    use async_openai::types::chat::{
        ChatCompletionRequestUserMessage, ChatCompletionRequestUserMessageContent,
    };
    use serde_json::json;
    use wiremock::matchers::body_partial_json;
    use wiremock::{Mock, MockServer, ResponseTemplate};

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
                Delta::Reasoning(text) => texts.push(format!("<reasoning>{text}")),
                Delta::ToolCall(chunk) => {
                    calls.push(chunk);
                }
                Delta::Truncated => texts.push("<truncated>".into()),
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

    #[test]
    fn a_ratio_pause_follows_the_request_time_up_to_its_max() {
        let secs = Duration::from_secs;
        let ratio = |max| RequestPause::Ratio { ratio: 0.5, max };

        assert_eq!(RequestPause::Fixed(secs(30)).after(secs(90)), secs(30));
        assert_eq!(ratio(None).after(secs(10)), secs(5));
        assert_eq!(ratio(None).after(secs(90)), secs(45));
        assert_eq!(ratio(Some(secs(30))).after(secs(90)), secs(30));
        let huge = |max| RequestPause::Ratio { ratio: 1e20, max };
        assert_eq!(huge(None).after(secs(90)), Duration::MAX);
        assert_eq!(huge(Some(secs(30))).after(secs(90)), secs(30));
    }

    #[tokio::test]
    async fn a_chunk_with_both_reasoning_fields_parses() {
        let chunks = [chunk(
            json!({"reasoning": "Hmm", "reasoning_content": "Hmm"}),
            Some("eos"),
        )];
        let (_server, client) = client_for(sse(&chunks)).await;

        assert_eq!(collect(&client).await.unwrap().0, ["<reasoning>Hmm"]);
    }

    #[tokio::test]
    async fn reasoning_streams_under_either_field_name() {
        let chunks = [
            chunk(json!({"reasoning_content": "Hmm"}), None),
            chunk(json!({"reasoning": ", so"}), None),
            chunk(json!({"content": "391"}), None),
        ];
        let (_server, client) = client_for(sse(&chunks)).await;

        assert_eq!(
            collect(&client).await.unwrap().0,
            ["<reasoning>Hmm", "<reasoning>, so", "391"]
        );
    }

    #[tokio::test]
    async fn requests_ask_for_at_most_max_tokens() {
        let server = MockServer::start().await;
        Mock::given(body_partial_json(json!({"max_completion_tokens": 4096})))
            .respond_with(reply(&["ok"]))
            .mount(&server)
            .await;
        let client = ModelClient::new(&config(api_url(&server)));

        assert_eq!(collect(&client).await.unwrap().0, ["ok"]);
    }

    #[tokio::test]
    async fn requests_send_the_reasoning_effort_only_when_set() {
        let server = MockServer::start().await;
        let client = ModelClient::new(&config(api_url(&server)));
        respond(&server, reply(&["ok"])).await;
        collect(&client).await.unwrap();
        let client = ModelClient::new(&ModelConfig {
            reasoning_effort: Some(ReasoningEffort::None),
            ..config(api_url(&server))
        });
        respond(&server, reply(&["ok"])).await;
        collect(&client).await.unwrap();

        let sent = server.received_requests().await.unwrap();
        let effort = |n: usize| {
            sent[n].body_json::<serde_json::Value>().unwrap()["reasoning_effort"].clone()
        };
        assert_eq!(effort(0), serde_json::Value::Null);
        assert_eq!(effort(1), "none");
    }

    #[tokio::test]
    async fn ollama_requests_use_max_tokens_which_it_honours() {
        let server = MockServer::start().await;
        Mock::given(body_partial_json(json!({"max_tokens": 4096})))
            .respond_with(reply(&["ok"]))
            .mount(&server)
            .await;
        let client = ModelClient::new(&ModelConfig {
            backend: Backend::Ollama,
            ..config(api_url(&server))
        });

        assert_eq!(collect(&client).await.unwrap().0, ["ok"]);
    }

    #[tokio::test]
    async fn a_reply_cut_at_the_limit_ends_with_truncated() {
        let chunks = [chunk(json!({"content": "Once upon"}), Some("length"))];
        let (_server, client) = client_for(sse(&chunks)).await;

        assert_eq!(
            collect(&client).await.unwrap().0,
            ["Once upon", "<truncated>"]
        );
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
