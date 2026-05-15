use anyhow::Result;
use async_openai::types::chat::{
    ChatCompletionMessageToolCall, ChatCompletionMessageToolCalls,
    ChatCompletionRequestAssistantMessage, ChatCompletionRequestMessage,
    ChatCompletionRequestToolMessage, ChatCompletionRequestToolMessageContent, ChatCompletionTools,
    FunctionCall,
};
use futures::StreamExt;
use std::path::PathBuf;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::{client::AxonClient, config::ToolApproval, event::AppEvent, tools::ToolExecutor};

const MAX_TURNS: usize = 10;

/// Spawn the inference task rooted at the current working directory.
pub fn spawn(
    client: AxonClient,
    messages: Vec<ChatCompletionRequestMessage>,
    tools: Vec<ChatCompletionTools>,
    tool_approval: ToolApproval,
    cancel: CancellationToken,
) -> mpsc::Receiver<AppEvent> {
    spawn_in(
        client,
        messages,
        tools,
        tool_approval,
        std::env::current_dir().unwrap_or_default(),
        cancel,
    )
}

/// Spawn the inference task rooted at `working_dir`. Useful for tests.
pub fn spawn_in(
    client: AxonClient,
    messages: Vec<ChatCompletionRequestMessage>,
    tools: Vec<ChatCompletionTools>,
    _tool_approval: ToolApproval,
    working_dir: impl Into<PathBuf>,
    cancel: CancellationToken,
) -> mpsc::Receiver<AppEvent> {
    let (tx, rx) = mpsc::channel(64);
    let executor = ToolExecutor::new(working_dir);
    tokio::spawn(async move {
        run(client, messages, tools, tx, executor, cancel).await;
    });
    rx
}

async fn run(
    client: AxonClient,
    mut messages: Vec<ChatCompletionRequestMessage>,
    tools: Vec<ChatCompletionTools>,
    tx: mpsc::Sender<AppEvent>,
    executor: ToolExecutor,
    cancel: CancellationToken,
) {
    for _ in 0..MAX_TURNS {
        let stream = client.stream_chat(messages.clone(), tools.clone());
        let tool_calls = tokio::select! {
            biased;
            _ = cancel.cancelled() => break,
            result = run_turn(stream, &tx) => match result {
                Ok(calls) => calls,
                Err(_) => break,
            },
        };

        if tool_calls.is_empty() {
            break;
        }

        // Append assistant message with the tool_calls to history.
        let msg_tool_calls: Vec<ChatCompletionMessageToolCalls> = tool_calls
            .iter()
            .map(|(id, name, args)| {
                ChatCompletionMessageToolCalls::Function(ChatCompletionMessageToolCall {
                    id: id.clone(),
                    function: FunctionCall {
                        name: name.clone(),
                        arguments: serde_json::to_string(args).unwrap_or_default(),
                    },
                })
            })
            .collect();

        #[allow(deprecated)]
        messages.push(ChatCompletionRequestMessage::Assistant(
            ChatCompletionRequestAssistantMessage {
                tool_calls: Some(msg_tool_calls),
                ..Default::default()
            },
        ));

        // Execute each tool and append results to history.
        for (id, name, args) in &tool_calls {
            let content = executor
                .execute(name, args)
                .unwrap_or_else(|e| format!("error: {e}"));

            if tx
                .send(AppEvent::ToolResult {
                    name: name.clone(),
                    content: content.clone(),
                })
                .await
                .is_err()
            {
                return;
            }

            messages.push(ChatCompletionRequestMessage::Tool(
                ChatCompletionRequestToolMessage {
                    content: ChatCompletionRequestToolMessageContent::Text(content),
                    tool_call_id: id.clone(),
                },
            ));
        }
    }

    let _ = tx.send(AppEvent::Done).await;
}

/// Process one streaming turn: forward Token and ToolCall events to `tx`,
/// return the tool calls collected as (id, name, args). Returns early (Ok)
/// if `tx` is closed.
pub(crate) async fn run_turn(
    stream: impl futures::Stream<Item = Result<AppEvent>>,
    tx: &mpsc::Sender<AppEvent>,
) -> Result<Vec<(String, String, serde_json::Value)>> {
    futures::pin_mut!(stream);
    let mut tool_calls: Vec<(String, String, serde_json::Value)> = Vec::new();

    while let Some(result) = stream.next().await {
        match result? {
            AppEvent::Token(t) => {
                if tx.send(AppEvent::Token(t)).await.is_err() {
                    return Ok(tool_calls);
                }
            }
            AppEvent::ToolCall { id, name, args } => {
                tool_calls.push((id.clone(), name.clone(), args.clone()));
                if tx
                    .send(AppEvent::ToolCall { id, name, args })
                    .await
                    .is_err()
                {
                    return Ok(tool_calls);
                }
            }
            AppEvent::Done | AppEvent::ToolResult { .. } | AppEvent::FileDiff { .. } => {}
        }
    }

    Ok(tool_calls)
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures::stream;
    use serde_json::json;

    #[tokio::test]
    async fn token_turn_forwards_all_tokens() {
        let (tx, mut rx) = mpsc::channel(16);
        let events = vec![
            Ok(AppEvent::Token("hello".into())),
            Ok(AppEvent::Token(" world".into())),
            Ok(AppEvent::Done),
        ];
        let tool_calls = run_turn(stream::iter(events), &tx).await.unwrap();
        drop(tx);
        assert!(tool_calls.is_empty());
        assert_eq!(rx.recv().await.unwrap(), AppEvent::Token("hello".into()));
        assert_eq!(rx.recv().await.unwrap(), AppEvent::Token(" world".into()));
        assert!(rx.recv().await.is_none());
    }

    #[tokio::test]
    async fn tool_call_turn_forwards_event_and_returns_calls() {
        let (tx, mut rx) = mpsc::channel(16);
        let events = vec![
            Ok(AppEvent::ToolCall {
                id: "call_1".into(),
                name: "read_file".into(),
                args: json!({"path": "/etc/hosts"}),
            }),
            Ok(AppEvent::Done),
        ];
        let tool_calls = run_turn(stream::iter(events), &tx).await.unwrap();
        assert_eq!(tool_calls.len(), 1);
        assert_eq!(tool_calls[0].0, "call_1");
        assert_eq!(tool_calls[0].1, "read_file");
        assert_eq!(tool_calls[0].2, json!({"path": "/etc/hosts"}));
        assert_eq!(
            rx.recv().await.unwrap(),
            AppEvent::ToolCall {
                id: "call_1".into(),
                name: "read_file".into(),
                args: json!({"path": "/etc/hosts"}),
            }
        );
    }

    #[tokio::test]
    async fn stream_error_propagates() {
        let (tx, _rx) = mpsc::channel(16);
        let events: Vec<Result<AppEvent>> = vec![
            Ok(AppEvent::Token("hi".into())),
            Err(anyhow::anyhow!("network error")),
        ];
        let result = run_turn(stream::iter(events), &tx).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn receiver_drop_exits_cleanly() {
        let (tx, rx) = mpsc::channel(16);
        drop(rx);
        let events: Vec<Result<AppEvent>> = vec![
            Ok(AppEvent::Token("hi".into())),
            Ok(AppEvent::Token("there".into())),
        ];
        let result = run_turn(stream::iter(events), &tx).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn cancel_during_pending_stream_exits_without_hang() {
        use futures::stream;
        use tokio::time::{Duration, timeout};
        use tokio_util::sync::CancellationToken;

        let cancel = CancellationToken::new();
        // A stream that never resolves — simulates waiting on the next SSE token.
        let never_stream = stream::pending::<Result<AppEvent>>();

        let cancel_clone = cancel.clone();
        let handle = tokio::spawn(async move {
            let (tx, _rx) = mpsc::channel(1);
            tokio::select! {
                biased;
                _ = cancel_clone.cancelled() => {}
                _ = run_turn(never_stream, &tx) => {}
            }
        });

        cancel.cancel();

        timeout(Duration::from_millis(100), handle)
            .await
            .expect("task did not exit within 100ms after cancel")
            .expect("task panicked");
    }
}
