//! `axon acp` as a client sees it: the real binary over stdio, with the
//! core's model requests answered by a mock server.

use std::time::Duration;

use agent_client_protocol::schema::ProtocolVersion;
use agent_client_protocol::schema::v1::{
    ContentBlock, InitializeRequest, NewSessionRequest, PromptRequest, SessionNotification,
    SessionUpdate, StopReason, TextContent, ToolCallStatus,
};
use agent_client_protocol::{AcpAgent, AcpAgentConfig, Client};
use serde_json::{Value, json};
use tokio::sync::mpsc;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

/// A streamed chat completion made of these chunk deltas, the last one
/// finishing with `finish`.
fn stream(deltas: &[Value], finish: &str) -> ResponseTemplate {
    let mut body = String::new();
    for (i, delta) in deltas.iter().enumerate() {
        let finish_reason = (i + 1 == deltas.len()).then_some(finish);
        let chunk = json!({
            "id": "c", "object": "chat.completion.chunk", "created": 0, "model": "m",
            "choices": [{"index": 0, "delta": delta, "finish_reason": finish_reason}]
        });
        body.push_str(&format!("data: {chunk}\n\n"));
    }
    body.push_str("data: [DONE]\n\n");
    ResponseTemplate::new(200)
        .insert_header("content-type", "text/event-stream")
        .set_body_string(body)
}

#[tokio::test]
async fn a_prompt_runs_a_turn_with_a_tool_call_through_the_binary() {
    let server = MockServer::start().await;
    let call = json!({"tool_calls": [{"index": 0, "id": "c1", "type": "function",
        "function": {"name": "read_file", "arguments": "{\"path\":\"notes.txt\"}"}}]});
    for response in [
        stream(&[call], "tool_calls"),
        stream(&[json!({"content": "Buy milk."})], "stop"),
    ] {
        Mock::given(method("POST"))
            .and(path("/v1/chat/completions"))
            .respond_with(response)
            .up_to_n_times(1)
            .mount(&server)
            .await;
    }
    let work = tempfile::tempdir().unwrap();
    std::fs::write(work.path().join("notes.txt"), "milk\n").unwrap();
    let sessions = tempfile::tempdir().unwrap();
    let agent = AcpAgent::new(
        AcpAgentConfig::new(env!("CARGO_BIN_EXE_axon"))
            .arg("acp")
            .env("AXON_BACKEND", "openai")
            .env("OPENAI_BASE_URL", format!("{}/v1", server.uri()))
            .env("AXON_MODEL", "m")
            .env("AXON_SESSIONS_DIR", sessions.path().to_str().unwrap()),
    );
    let (update_tx, mut updates) = mpsc::unbounded_channel();
    let cwd = work.path().canonicalize().unwrap();

    let stop = tokio::time::timeout(
        Duration::from_secs(30),
        Client
            .builder()
            .on_receive_notification(
                async move |notification: SessionNotification, _connection| {
                    let _ = update_tx.send(notification.update);
                    Ok(())
                },
                agent_client_protocol::on_receive_notification!(),
            )
            .connect_with(agent, async |connection| {
                connection
                    .send_request(InitializeRequest::new(ProtocolVersion::V1))
                    .block_task()
                    .await?;
                let session = connection
                    .send_request(NewSessionRequest::new(cwd))
                    .block_task()
                    .await?
                    .session_id;
                let prompt = vec![ContentBlock::Text(TextContent::new("what's in my notes?"))];
                let response = connection
                    .send_request(PromptRequest::new(session, prompt))
                    .block_task()
                    .await?;
                Ok(response.stop_reason)
            }),
    )
    .await
    .expect("timed out")
    .unwrap();

    assert_eq!(stop, StopReason::EndTurn);
    let mut seen = Vec::new();
    while let Ok(update) = updates.try_recv() {
        seen.push(match update {
            SessionUpdate::ToolCall(call) => format!("call {} {:?}", call.title, call.status),
            SessionUpdate::ToolCallUpdate(update) => {
                format!(
                    "update {:?} {:?}",
                    update.fields.status, update.fields.content
                )
            }
            SessionUpdate::AgentMessageChunk(chunk) => match chunk.content {
                ContentBlock::Text(text) => format!("message {}", text.text),
                other => format!("message {other:?}"),
            },
            other => format!("{other:?}"),
        });
    }
    assert_eq!(seen.len(), 3, "{seen:#?}");
    assert_eq!(
        seen[0],
        format!("call read_file {:?}", ToolCallStatus::InProgress)
    );
    assert!(
        seen[1].starts_with(&format!("update Some({:?})", ToolCallStatus::Completed)),
        "{}",
        seen[1]
    );
    assert!(seen[1].contains("milk"), "{}", seen[1]);
    assert_eq!(seen[2], "message Buy milk.");
}
