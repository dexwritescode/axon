//! A mock OpenAI-compatible server for the tests.

use serde_json::{Value, json};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

use std::path::Path;
use std::time::Duration;

use axon_protocol::{Command, Event, SessionHandle, StopReason};
use tokio::sync::mpsc;

use crate::backend::Backend;
use crate::model::{ModelClient, ModelConfig};
use crate::turn::Agent;

pub(crate) fn config(base_url: String) -> ModelConfig {
    ModelConfig {
        backend: Backend::LmStudio,
        base_url,
        api_key: "test".into(),
        model: "test-model".into(),
        request_pause: Duration::ZERO,
        max_tokens: 4096,
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

/// A 400. Not a 5xx: async-openai's default retry layer retries those with backoff.
pub(crate) fn client_error() -> ResponseTemplate {
    ResponseTemplate::new(400)
}

/// Answer the next chat completion request with `response`.
pub(crate) async fn respond(server: &MockServer, response: ResponseTemplate) {
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(response)
        .up_to_n_times(1)
        .mount(server)
        .await;
}

/// A 200 SSE response in which the model makes these `(id, name, args)` tool
/// calls, each one's arguments split across two chunks.
pub(crate) fn tool_calls(calls: &[(&str, &str, &str)]) -> ResponseTemplate {
    let mut chunks = Vec::new();
    for (index, (id, name, args)) in calls.iter().enumerate() {
        let (head, tail) = args.split_at(args.len() / 2);
        chunks.push(chunk(
            json!({"tool_calls": [{"index": index, "id": id, "type": "function",
                "function": {"name": name, "arguments": head}}]}),
            None,
        ));
        chunks.push(chunk(
            json!({"tool_calls": [{"index": index, "function": {"arguments": tail}}]}),
            None,
        ));
    }
    chunks.push(chunk(json!({}), Some("tool_calls")));
    sse(&chunks)
}

/// An agent talking to `server`, working in `cwd`.
pub(crate) fn agent(server: &MockServer, cwd: &Path) -> Agent {
    Agent {
        client: ModelClient::new(&config(api_url(server))),
        cwd: cwd.canonicalize().unwrap(),
        max_requests: 25,
        request_pause: Duration::ZERO,
        seen: Default::default(),
    }
}

/// Start a session with no context limit, past its first event and without
/// tool-call progress events, which only `turn`'s progress test checks.
pub(crate) async fn spawn_started(agent: Agent) -> SessionHandle {
    let SessionHandle {
        commands,
        events: mut all,
    } = crate::session::spawn(agent, async { None });
    assert_eq!(all.recv().await, Some(Event::ContextLimit { tokens: None }));
    let (tx, events) = mpsc::channel(crate::session::EVENT_BUFFER);
    tokio::spawn(async move {
        loop {
            let event = tokio::select! {
                _ = tx.closed() => break,
                event = all.recv() => event,
            };
            match event {
                Some(Event::ToolCallStreaming { .. }) => {}
                Some(event) => {
                    if tx.send(event).await.is_err() {
                        break;
                    }
                }
                None => break,
            }
        }
    });
    SessionHandle { commands, events }
}

pub(crate) fn end(stop: StopReason) -> Event {
    Event::TurnEnded { stop }
}

pub(crate) fn delta(text: &str) -> Event {
    Event::TextDelta { text: text.into() }
}

/// Send a prompt and collect events after its `UserMessage`, up to and
/// including `TurnEnded`.
pub(crate) async fn prompt(handle: &mut SessionHandle, text: &str) -> Vec<Event> {
    prompt_answering(handle, text, None).await
}

/// Like `prompt`, answering every `ApprovalRequested` with `approve`, if given.
pub(crate) async fn prompt_answering(
    handle: &mut SessionHandle,
    text: &str,
    approve: Option<bool>,
) -> Vec<Event> {
    handle
        .commands
        .send(Command::Prompt { text: text.into() })
        .await
        .unwrap();
    let first = handle.events.recv().await;
    assert_eq!(first, Some(Event::UserMessage { text: text.into() }));
    let mut events = Vec::new();
    while let Some(event) = handle.events.recv().await {
        if let (Event::ApprovalRequested { id, .. }, Some(approve)) = (&event, approve) {
            let answer = Command::Approval { id: *id, approve };
            handle.commands.send(answer).await.unwrap();
        }
        let done = matches!(event, Event::TurnEnded { .. });
        events.push(event);
        if done {
            break;
        }
    }
    events
}

/// The `messages` of each request the server received, after the system prompt.
pub(crate) async fn sent_messages(server: &MockServer) -> Vec<Value> {
    server
        .received_requests()
        .await
        .unwrap()
        .iter()
        .map(|request| {
            let body: Value = request.body_json().unwrap();
            let messages = body["messages"].as_array().unwrap();
            assert_eq!(messages[0]["role"], "system");
            Value::Array(messages[1..].to_vec())
        })
        .collect()
}
