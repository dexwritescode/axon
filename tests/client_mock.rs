use async_openai::types::chat::{
    ChatCompletionRequestMessage, ChatCompletionRequestUserMessage,
    ChatCompletionRequestUserMessageContent,
};
use axon::client::AxonClient;
use axon::config::{BackendConfig, ToolApproval};
use axon::event::AppEvent;
use futures::StreamExt;
use serde_json::json;
use tokio_util::sync::CancellationToken;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn backend(base_url: &str) -> BackendConfig {
    BackendConfig {
        base_url: base_url.to_string(),
        api_key: "test".to_string(),
        model: "test-model".to_string(),
    }
}

fn sse_body(data_lines: &[&str]) -> Vec<u8> {
    let mut body = data_lines
        .iter()
        .map(|d| format!("data: {d}\n\n"))
        .collect::<String>();
    body.push_str("data: [DONE]\n\n");
    body.into_bytes()
}

fn user_message(text: &str) -> ChatCompletionRequestMessage {
    ChatCompletionRequestMessage::User(ChatCompletionRequestUserMessage {
        content: ChatCompletionRequestUserMessageContent::Text(text.to_string()),
        name: None,
    })
}

async fn collect_events(server: &MockServer, body: Vec<u8>) -> Vec<AppEvent> {
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "text/event-stream")
                .set_body_bytes(body),
        )
        .mount(server)
        .await;

    // server.uri() is "http://127.0.0.1:PORT" — append /v1 to match async-openai's path appending
    let client = AxonClient::new(&backend(&format!("{}/v1", server.uri())));
    client
        .stream_chat(vec![user_message("test")], vec![])
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .map(|r| r.expect("stream error"))
        .collect()
}

fn chunk(content: &str) -> String {
    serde_json::to_string(&json!({
        "id": "chatcmpl-test",
        "object": "chat.completion.chunk",
        "created": 0,
        "model": "test-model",
        "choices": [{"index": 0, "delta": {"content": content}, "finish_reason": null}]
    }))
    .unwrap()
}

fn tool_chunk(index: u32, id: Option<&str>, name: Option<&str>, args: Option<&str>) -> String {
    let mut tc = json!({"index": index});
    if let Some(id) = id {
        tc["id"] = json!(id);
        tc["type"] = json!("function");
    }
    let mut func = json!({});
    if let Some(n) = name {
        func["name"] = json!(n);
    }
    if let Some(a) = args {
        func["arguments"] = json!(a);
    }
    tc["function"] = func;

    serde_json::to_string(&json!({
        "id": "chatcmpl-test",
        "object": "chat.completion.chunk",
        "created": 0,
        "model": "test-model",
        "choices": [{"index": 0, "delta": {"tool_calls": [tc]}, "finish_reason": null}]
    }))
    .unwrap()
}

fn stop_chunk() -> String {
    serde_json::to_string(&json!({
        "id": "chatcmpl-test",
        "object": "chat.completion.chunk",
        "created": 0,
        "model": "test-model",
        "choices": [{"index": 0, "delta": {}, "finish_reason": "stop"}]
    }))
    .unwrap()
}

#[tokio::test]
async fn token_stream() {
    let server = MockServer::start().await;
    let body = sse_body(&[
        &chunk("Hello"),
        &chunk(", "),
        &chunk("world"),
        &stop_chunk(),
    ]);
    let events = collect_events(&server, body).await;
    assert_eq!(
        events,
        vec![
            AppEvent::Token("Hello".into()),
            AppEvent::Token(", ".into()),
            AppEvent::Token("world".into()),
            AppEvent::Done,
        ]
    );
}

#[tokio::test]
async fn tool_call_single_chunk() {
    let server = MockServer::start().await;
    // All args arrive in the first chunk; subsequent chunk just has finish_reason.
    let first = tool_chunk(
        0,
        Some("call_abc"),
        Some("read_file"),
        Some(r#"{"path":"/etc/hostname"}"#),
    );
    let finish = serde_json::to_string(&json!({
        "id": "chatcmpl-test",
        "object": "chat.completion.chunk",
        "created": 0,
        "model": "test-model",
        "choices": [{"index": 0, "delta": {}, "finish_reason": "tool_calls"}]
    }))
    .unwrap();
    let body = sse_body(&[&first, &finish]);
    let events = collect_events(&server, body).await;
    assert_eq!(
        events,
        vec![
            AppEvent::ToolCall {
                id: "call_abc".into(),
                name: "read_file".into(),
                args: json!({"path": "/etc/hostname"}),
            },
            AppEvent::Done,
        ]
    );
}

#[tokio::test]
async fn tool_call_fragmented_args() {
    let server = MockServer::start().await;
    // Name + id arrive in chunk 1; args split across chunks 2-4.
    let c1 = tool_chunk(0, Some("call_abc"), Some("read_file"), Some(""));
    let c2 = tool_chunk(0, None, None, Some(r#"{"pa"#));
    let c3 = tool_chunk(0, None, None, Some(r#"th":""#));
    let c4 = tool_chunk(0, None, None, Some(r#"/etc/hostname"}"#));
    let finish = serde_json::to_string(&json!({
        "id": "chatcmpl-test",
        "object": "chat.completion.chunk",
        "created": 0,
        "model": "test-model",
        "choices": [{"index": 0, "delta": {}, "finish_reason": "tool_calls"}]
    }))
    .unwrap();
    let body = sse_body(&[&c1, &c2, &c3, &c4, &finish]);
    let events = collect_events(&server, body).await;
    assert_eq!(
        events,
        vec![
            AppEvent::ToolCall {
                id: "call_abc".into(),
                name: "read_file".into(),
                args: json!({"path": "/etc/hostname"}),
            },
            AppEvent::Done,
        ]
    );
}

#[tokio::test]
async fn mixed_token_and_tool_call() {
    let server = MockServer::start().await;
    // Two token deltas followed by a tool call.
    let t1 = chunk("Thinking");
    let t2 = chunk("...");
    let tc = tool_chunk(0, Some("call_1"), Some("shell"), Some(r#"{"cmd":"ls"}"#));
    let finish = serde_json::to_string(&json!({
        "id": "chatcmpl-test",
        "object": "chat.completion.chunk",
        "created": 0,
        "model": "test-model",
        "choices": [{"index": 0, "delta": {}, "finish_reason": "tool_calls"}]
    }))
    .unwrap();
    let body = sse_body(&[&t1, &t2, &tc, &finish]);
    let events = collect_events(&server, body).await;
    assert_eq!(
        events,
        vec![
            AppEvent::Token("Thinking".into()),
            AppEvent::Token("...".into()),
            AppEvent::ToolCall {
                id: "call_1".into(),
                name: "shell".into(),
                args: json!({"cmd": "ls"}),
            },
            AppEvent::Done,
        ]
    );
}

#[tokio::test]
async fn cancel_mid_stream_exits_without_hang() {
    use tokio::time::{Duration, timeout};

    let server = MockServer::start().await;

    // One token then no [DONE] — the stream hangs indefinitely.
    let mut body = format!("data: {}\n\n", chunk("hello"));
    body.push_str("data: [INCOMPLETE]");

    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "text/event-stream")
                .set_body_bytes(body.into_bytes()),
        )
        .mount(&server)
        .await;

    let cancel = CancellationToken::new();
    let mut rx = axon::inference::spawn_in(
        AxonClient::new(&backend(&format!("{}/v1", server.uri()))),
        vec![user_message("test")],
        vec![],
        ToolApproval::Allow,
        std::env::temp_dir(),
        cancel.clone(),
    );

    // Wait for at least one token so the stream has started.
    let first = timeout(Duration::from_secs(2), rx.recv())
        .await
        .expect("timed out waiting for first token");
    assert!(matches!(first, Some(AppEvent::Token(_))));

    // Cancel and assert the channel closes promptly.
    cancel.cancel();
    let closed = timeout(Duration::from_millis(500), async {
        while rx.recv().await.is_some() {}
    })
    .await;
    assert!(
        closed.is_ok(),
        "inference task did not exit within 500ms after cancel"
    );
}
