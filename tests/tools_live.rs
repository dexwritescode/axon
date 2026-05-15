//! End-to-end integration tests for tool execution against a real neuron server.
//! Skipped automatically when AXON_TEST_BASE_URL is not set.
//!
//! To run locally:
//!   neuron server -m mlx-community/Qwen3-8B-4bit --http-port 8080 &
//!   AXON_TEST_BASE_URL=http://127.0.0.1:8080/v1 \
//!   AXON_TEST_MODEL=mlx-community/Qwen3-8B-4bit \
//!   cargo test --test tools_live -- --nocapture --test-threads=1

use std::time::Duration;
use std::{fs, path::Path};

use async_openai::types::chat::{
    ChatCompletionRequestMessage, ChatCompletionRequestUserMessageArgs,
};
use axon::{client::AxonClient, config::BackendConfig, event::AppEvent, tools::tool_schemas};
use tempfile::TempDir;
use tokio::sync::mpsc;
use tokio::time::timeout;

fn live_client() -> Option<(AxonClient, String)> {
    let base_url = std::env::var("AXON_TEST_BASE_URL").ok()?;
    let model = std::env::var("AXON_TEST_MODEL")
        .unwrap_or_else(|_| "mlx-community/Qwen3-8B-4bit".to_string());
    let client = AxonClient::new(&BackendConfig {
        base_url,
        api_key: std::env::var("OPENAI_API_KEY").unwrap_or_else(|_| "x".to_string()),
        model: model.clone(),
    });
    Some((client, model))
}

fn user_msg(text: &str) -> ChatCompletionRequestMessage {
    ChatCompletionRequestMessage::User(
        ChatCompletionRequestUserMessageArgs::default()
            .content(text)
            .build()
            .unwrap(),
    )
}

/// Collect all AppEvents from the inference task, with a timeout.
async fn collect(rx: &mut mpsc::Receiver<AppEvent>) -> Vec<AppEvent> {
    let mut events = Vec::new();
    loop {
        match timeout(Duration::from_secs(120), rx.recv()).await {
            Ok(Some(ev)) => {
                let done = matches!(ev, AppEvent::Done);
                events.push(ev);
                if done {
                    break;
                }
            }
            Ok(None) | Err(_) => break,
        }
    }
    events
}

fn has_tool_call(events: &[AppEvent], tool_name: &str) -> bool {
    events
        .iter()
        .any(|e| matches!(e, AppEvent::ToolCall { name, .. } if name == tool_name))
}

fn has_tool_result(events: &[AppEvent], tool_name: &str) -> bool {
    events
        .iter()
        .any(|e| matches!(e, AppEvent::ToolResult { name, .. } if name == tool_name))
}

fn token_text(events: &[AppEvent]) -> String {
    events
        .iter()
        .filter_map(|e| {
            if let AppEvent::Token(t) = e {
                Some(t.as_str())
            } else {
                None
            }
        })
        .collect()
}

// ── read_file ────────────────────────────────────────────────────────────────

#[tokio::test]
async fn read_file_tool_called_and_executed() {
    let Some((client, _)) = live_client() else {
        return;
    };

    let dir = TempDir::new().unwrap();
    let file_path = dir.path().join("greeting.txt");
    fs::write(&file_path, "Hello from Axon!").unwrap();

    let prompt = "Read the file 'greeting.txt' and tell me exactly what it contains. \
                  The file is in the current directory.";

    let (tx, mut rx) = mpsc::channel(64);
    let working_dir = dir.path().to_owned();

    tokio::spawn(async move {
        run_agent(
            client,
            vec![user_msg(prompt)],
            tool_schemas(),
            tx,
            &working_dir,
        )
        .await;
    });

    let events = collect(&mut rx).await;

    assert!(
        has_tool_call(&events, "read_file"),
        "expected read_file tool call"
    );
    assert!(
        has_tool_result(&events, "read_file"),
        "expected read_file result"
    );

    let text = token_text(&events);
    assert!(
        text.contains("Hello from Axon!"),
        "expected file contents in final response, got: {text}"
    );
}

// ── edit_file ────────────────────────────────────────────────────────────────

#[tokio::test]
async fn edit_file_tool_called_and_file_written() {
    let Some((client, _)) = live_client() else {
        return;
    };

    let dir = TempDir::new().unwrap();
    let out_path = dir.path().join("output.txt");
    let working_dir = dir.path().to_owned();

    let prompt = "Write the text 'axon-edit-test' to the file 'output.txt'.";

    let (tx, mut rx) = mpsc::channel(64);

    tokio::spawn(async move {
        run_agent(
            client,
            vec![user_msg(prompt)],
            tool_schemas(),
            tx,
            &working_dir,
        )
        .await;
    });

    let events = collect(&mut rx).await;

    assert!(
        has_tool_call(&events, "edit_file"),
        "expected edit_file tool call"
    );
    assert!(
        has_tool_result(&events, "edit_file"),
        "expected edit_file result"
    );
    assert!(out_path.exists(), "expected file to be created on disk");

    let contents = fs::read_to_string(&out_path).unwrap();
    assert!(
        contents.contains("axon-edit-test"),
        "expected 'axon-edit-test' in written file, got: {contents}"
    );
}

// ── shell ────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn shell_tool_called_and_output_returned() {
    let Some((client, _)) = live_client() else {
        return;
    };

    let dir = TempDir::new().unwrap();
    let working_dir = dir.path().to_owned();
    let prompt = "Run the shell command 'echo axon-shell-test' and tell me the output.";

    let (tx, mut rx) = mpsc::channel(64);

    tokio::spawn(async move {
        run_agent(
            client,
            vec![user_msg(prompt)],
            tool_schemas(),
            tx,
            &working_dir,
        )
        .await;
    });

    let events = collect(&mut rx).await;

    assert!(has_tool_call(&events, "shell"), "expected shell tool call");
    assert!(has_tool_result(&events, "shell"), "expected shell result");

    let text = token_text(&events);
    assert!(
        text.contains("axon-shell-test"),
        "expected shell output in final response, got: {text}"
    );
}

// ── multi-step ───────────────────────────────────────────────────────────────

#[tokio::test]
async fn multi_step_read_then_shell_verify() {
    let Some((client, _)) = live_client() else {
        return;
    };

    let dir = TempDir::new().unwrap();
    fs::write(dir.path().join("data.txt"), "multi-step-content").unwrap();
    let working_dir = dir.path().to_owned();

    let prompt = "Read 'data.txt', then run 'cat data.txt' to verify it, \
                  and tell me what both returned.";

    let (tx, mut rx) = mpsc::channel(64);

    tokio::spawn(async move {
        run_agent(
            client,
            vec![user_msg(prompt)],
            tool_schemas(),
            tx,
            &working_dir,
        )
        .await;
    });

    let events = collect(&mut rx).await;

    let tool_calls: Vec<&str> = events
        .iter()
        .filter_map(|e| {
            if let AppEvent::ToolCall { name, .. } = e {
                Some(name.as_str())
            } else {
                None
            }
        })
        .collect();

    assert!(
        tool_calls.len() >= 2,
        "expected at least two tool calls, got: {tool_calls:?}"
    );
    assert!(
        tool_calls.contains(&"read_file") || tool_calls.contains(&"shell"),
        "expected read_file and/or shell calls"
    );
}

// ── helpers ───────────────────────────────────────────────────────────────────

/// Minimal agent loop: infer → execute tools → reinject → repeat until Done.
async fn run_agent(
    client: AxonClient,
    messages: Vec<ChatCompletionRequestMessage>,
    tools: Vec<async_openai::types::chat::ChatCompletionTools>,
    tx: mpsc::Sender<AppEvent>,
    working_dir: &Path,
) {
    use axon::config::ToolApproval;
    use tokio_util::sync::CancellationToken;
    let mut rx = axon::inference::spawn_in(
        client,
        messages,
        tools,
        ToolApproval::Allow,
        working_dir,
        CancellationToken::new(),
    );
    while let Some(ev) = rx.recv().await {
        let done = matches!(ev, AppEvent::Done);
        let _ = tx.send(ev).await;
        if done {
            break;
        }
    }
}
