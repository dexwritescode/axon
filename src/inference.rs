use anyhow::Result;
use async_openai::types::chat::{ChatCompletionRequestMessage, ChatCompletionTools};
use futures::StreamExt;
use tokio::sync::mpsc;

use crate::{client::AxonClient, config::ToolApproval, event::AppEvent};

/// Spawn the inference task and return the event receiver.
///
/// Drop the receiver to cancel: the task detects the closed channel and exits
/// at the next send, so no explicit abort is needed.
pub fn spawn(
    client: AxonClient,
    messages: Vec<ChatCompletionRequestMessage>,
    tools: Vec<ChatCompletionTools>,
    _tool_approval: ToolApproval,
) -> mpsc::Receiver<AppEvent> {
    let (tx, rx) = mpsc::channel(64);
    tokio::spawn(async move {
        run(client, messages, tools, tx).await;
    });
    rx
}

async fn run(
    client: AxonClient,
    messages: Vec<ChatCompletionRequestMessage>,
    tools: Vec<ChatCompletionTools>,
    tx: mpsc::Sender<AppEvent>,
) {
    let stream = client.stream_chat(messages, tools);
    match run_turn(stream, &tx).await {
        Ok(tool_calls) if !tool_calls.is_empty() => {
            // Stub: acknowledge each tool call with a placeholder result.
            // Real dispatch to the local executor lands in axon-je8.
            for (name, _args) in tool_calls {
                if tx
                    .send(AppEvent::ToolResult {
                        name,
                        content: "stub".into(),
                    })
                    .await
                    .is_err()
                {
                    return; // receiver dropped
                }
            }
        }
        Ok(_) | Err(_) => {}
    }
    let _ = tx.send(AppEvent::Done).await;
}

/// Process one streaming turn: forward Token and ToolCall events to `tx`,
/// return the tool calls collected. Returns early (Ok) if `tx` is closed.
pub(crate) async fn run_turn(
    stream: impl futures::Stream<Item = Result<AppEvent>>,
    tx: &mpsc::Sender<AppEvent>,
) -> Result<Vec<(String, serde_json::Value)>> {
    futures::pin_mut!(stream);
    let mut tool_calls = Vec::new();

    while let Some(result) = stream.next().await {
        match result? {
            AppEvent::Token(t) => {
                if tx.send(AppEvent::Token(t)).await.is_err() {
                    return Ok(tool_calls);
                }
            }
            AppEvent::ToolCall { name, args } => {
                tool_calls.push((name.clone(), args.clone()));
                if tx.send(AppEvent::ToolCall { name, args }).await.is_err() {
                    return Ok(tool_calls);
                }
            }
            AppEvent::Done | AppEvent::ToolResult { .. } => {}
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
        assert!(rx.recv().await.is_none()); // Done is not forwarded; channel closed by drop(tx)
    }

    #[tokio::test]
    async fn tool_call_turn_forwards_event_and_returns_calls() {
        let (tx, mut rx) = mpsc::channel(16);
        let events = vec![
            Ok(AppEvent::ToolCall {
                name: "read_file".into(),
                args: json!({"path": "/etc/hosts"}),
            }),
            Ok(AppEvent::Done),
        ];
        let tool_calls = run_turn(stream::iter(events), &tx).await.unwrap();
        assert_eq!(tool_calls.len(), 1);
        assert_eq!(tool_calls[0].0, "read_file");
        assert_eq!(tool_calls[0].1, json!({"path": "/etc/hosts"}));
        assert_eq!(
            rx.recv().await.unwrap(),
            AppEvent::ToolCall {
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
        drop(rx); // close the receiving end immediately
        let events: Vec<Result<AppEvent>> = vec![
            Ok(AppEvent::Token("hi".into())),
            Ok(AppEvent::Token("there".into())),
        ];
        // run_turn must return Ok (not panic or hang) when the receiver is gone
        let result = run_turn(stream::iter(events), &tx).await;
        assert!(result.is_ok());
    }
}
