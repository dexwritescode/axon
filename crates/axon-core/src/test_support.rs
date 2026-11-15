//! A mock OpenAI-compatible server for the tests.

use serde_json::{Value, json};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

use crate::model::ModelConfig;

pub(crate) fn config(base_url: String) -> ModelConfig {
    ModelConfig {
        base_url,
        api_key: "test".into(),
        model: "test-model".into(),
    }
}

/// The API base URL of `server`.
pub(crate) fn api_url(server: &MockServer) -> String {
    format!("{}/v1", server.uri())
}

/// One streamed chat completion chunk.
pub(crate) fn chunk(delta: Value, finish_reason: Option<&str>) -> String {
    json!({
        "id": "chatcmpl-test",
        "object": "chat.completion.chunk",
        "created": 0,
        "model": "test-model",
        "choices": [{"index": 0, "delta": delta, "finish_reason": finish_reason}]
    })
    .to_string()
}

/// A 200 SSE response streaming these chunks, then `[DONE]`.
pub(crate) fn sse(chunks: &[String]) -> ResponseTemplate {
    let mut body: String = chunks.iter().map(|c| format!("data: {c}\n\n")).collect();
    body.push_str("data: [DONE]\n\n");
    ResponseTemplate::new(200)
        .insert_header("content-type", "text/event-stream")
        .set_body_string(body)
}

/// A 200 SSE response streaming these text deltas.
pub(crate) fn reply(deltas: &[&str]) -> ResponseTemplate {
    let chunks: Vec<String> = deltas
        .iter()
        .map(|delta| chunk(json!({"content": delta}), None))
        .collect();
    sse(&chunks)
}

/// Answer the next `times` chat completion requests with `response`.
pub(crate) async fn respond(server: &MockServer, response: ResponseTemplate, times: u64) {
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(response)
        .up_to_n_times(times)
        .mount(server)
        .await;
}
