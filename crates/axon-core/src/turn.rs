//! The agent loop for one prompt: call the model, run its tool calls, repeat.

use std::path::PathBuf;
use std::pin::pin;

use anyhow::Result;
use async_openai::types::chat::{
    ChatCompletionMessageToolCall, ChatCompletionMessageToolCalls,
    ChatCompletionRequestAssistantMessageArgs, ChatCompletionRequestMessage,
    ChatCompletionRequestSystemMessage, ChatCompletionRequestToolMessageArgs, FunctionCall,
};
use axon_protocol::{Event, StopReason, ToolStatus};
use futures::StreamExt;
use tokio::sync::mpsc;

use crate::model::{Delta, ModelClient, ToolCall, ToolCalls};
use crate::{prompt, tools};

/// What a turn needs besides the history.
pub(crate) struct Agent {
    pub(crate) client: ModelClient,
    /// Canonical; tools can't reach outside it.
    pub(crate) cwd: PathBuf,
    /// Model requests per prompt before the turn stops.
    pub(crate) max_requests: usize,
}

impl Agent {
    /// Run one turn, adding its messages to `history`.
    pub(crate) async fn turn(
        &self,
        history: &mut Vec<ChatCompletionRequestMessage>,
        events: &mpsc::Sender<Event>,
    ) -> Result<StopReason> {
        let system = ChatCompletionRequestSystemMessage::from(prompt::system(&self.cwd));
        for _ in 0..self.max_requests {
            let messages = std::iter::once(system.clone().into())
                .chain(history.iter().cloned())
                .collect();
            let (text, calls) = self.stream_reply(messages, events).await?;
            history.push(assistant_message(text, &calls)?);
            if calls.is_empty() {
                return Ok(StopReason::EndTurn);
            }
            for call in calls {
                let output = self.run_tool(&call, events).await?;
                history.push(
                    ChatCompletionRequestToolMessageArgs::default()
                        .content(output)
                        .tool_call_id(call.id)
                        .build()?
                        .into(),
                );
            }
        }
        Ok(StopReason::MaxTurnRequests)
    }

    /// Stream one model reply, forwarding its text; returns the text and tool calls.
    async fn stream_reply(
        &self,
        messages: Vec<ChatCompletionRequestMessage>,
        events: &mpsc::Sender<Event>,
    ) -> Result<(String, Vec<ToolCall>)> {
        let mut stream = pin!(self.client.stream_turn(messages, tools::schemas()).await?);
        let mut text = String::new();
        let mut calls = ToolCalls::default();
        while let Some(delta) = stream.next().await {
            match delta? {
                Delta::Text(delta) => {
                    text.push_str(&delta);
                    events.send(Event::TextDelta { text: delta }).await?;
                }
                Delta::ToolCall(chunk) => calls.push(chunk),
            }
        }
        Ok((text, calls.finish()))
    }

    /// Run a tool call, reporting it to the frontend; returns what the model sees.
    async fn run_tool(&self, call: &ToolCall, events: &mpsc::Sender<Event>) -> Result<String> {
        events
            .send(Event::ToolCallStarted {
                name: call.name.clone(),
                args: call.args.clone(),
            })
            .await?;
        let (cwd, name, args) = (self.cwd.clone(), call.name.clone(), call.args.clone());
        let result = tokio::task::spawn_blocking(move || tools::run(&cwd, &name, &args)).await?;
        let (status, output) = match result {
            Ok(output) => (ToolStatus::Completed, output),
            Err(error) => (ToolStatus::Failed, format!("error: {error}")),
        };
        events
            .send(Event::ToolCallFinished {
                name: call.name.clone(),
                status,
                output: output.clone(),
            })
            .await?;
        Ok(output)
    }
}

fn assistant_message(text: String, calls: &[ToolCall]) -> Result<ChatCompletionRequestMessage> {
    let mut message = ChatCompletionRequestAssistantMessageArgs::default();
    if !text.is_empty() || calls.is_empty() {
        message.content(text);
    }
    if !calls.is_empty() {
        message.tool_calls(
            calls
                .iter()
                .map(|call| {
                    ChatCompletionMessageToolCalls::Function(ChatCompletionMessageToolCall {
                        id: call.id.clone(),
                        function: FunctionCall {
                            name: call.name.clone(),
                            arguments: call.args.clone(),
                        },
                    })
                })
                .collect::<Vec<_>>(),
        );
    }
    Ok(message.build()?.into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session;
    use crate::test_support::{agent, prompt, reply, respond, sent_messages, tool_calls};
    use axon_protocol::SessionHandle;
    use serde_json::json;
    use std::fs;
    use tempfile::TempDir;
    use wiremock::MockServer;

    /// A session working in a temp dir that holds `notes.txt`.
    async fn session_with_notes(max_requests: usize) -> (MockServer, TempDir, SessionHandle) {
        let server = MockServer::start().await;
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join("notes.txt"), "remember the milk\n").unwrap();
        let handle = session::spawn(Agent {
            max_requests,
            ..agent(&server, dir.path())
        });
        (server, dir, handle)
    }

    fn end(stop: StopReason) -> Event {
        Event::TurnEnded { stop }
    }

    #[tokio::test]
    async fn tool_result_goes_back_to_the_model_which_then_answers() {
        let (server, _dir, mut handle) = session_with_notes(25).await;
        let args = r#"{"path":"notes.txt"}"#;
        respond(&server, tool_calls(&[("call_1", "read_file", args)])).await;
        respond(&server, reply(&["Milk."])).await;

        assert_eq!(
            prompt(&mut handle, "what's in my notes?").await,
            [
                Event::ToolCallStarted {
                    name: "read_file".into(),
                    args: args.into()
                },
                Event::ToolCallFinished {
                    name: "read_file".into(),
                    status: ToolStatus::Completed,
                    output: "remember the milk\n".into()
                },
                Event::TextDelta {
                    text: "Milk.".into()
                },
                end(StopReason::EndTurn),
            ]
        );
        assert_eq!(
            sent_messages(&server).await[1],
            json!([
                {"role": "user", "content": "what's in my notes?"},
                {"role": "assistant", "tool_calls": [{"id": "call_1", "type": "function",
                    "function": {"name": "read_file", "arguments": args}}]},
                {"role": "tool", "content": "remember the milk\n", "tool_call_id": "call_1"}
            ])
        );
    }

    #[tokio::test]
    async fn every_request_starts_with_a_system_prompt_naming_the_cwd() {
        let (server, dir, mut handle) = session_with_notes(25).await;
        respond(&server, reply(&["Hi"])).await;

        prompt(&mut handle, "hi").await;

        let request = &server.received_requests().await.unwrap()[0];
        let body: serde_json::Value = request.body_json().unwrap();
        let cwd = dir.path().canonicalize().unwrap();
        assert!(
            body["messages"][0]["content"]
                .as_str()
                .unwrap()
                .contains(cwd.to_str().unwrap())
        );
        assert_eq!(body["tools"].as_array().unwrap().len(), 2);
    }

    #[tokio::test]
    async fn failing_tool_reports_the_error_to_frontend_and_model() {
        let (server, _dir, mut handle) = session_with_notes(25).await;
        respond(&server, tool_calls(&[("c1", "read_file", "{not json}")])).await;
        respond(&server, reply(&["Sorry."])).await;

        let events = prompt(&mut handle, "read").await;

        let Event::ToolCallFinished { status, output, .. } = &events[1] else {
            panic!("{events:?}");
        };
        assert_eq!(*status, ToolStatus::Failed);
        assert!(
            output.starts_with("error: arguments were not valid JSON"),
            "{output}"
        );
        assert_eq!(
            sent_messages(&server).await[1][2]["content"],
            output.as_str()
        );
    }

    #[tokio::test]
    async fn turn_stops_at_the_request_limit() {
        let (server, _dir, mut handle) = session_with_notes(2).await;
        for _ in 0..3 {
            respond(&server, tool_calls(&[("c", "list_dir", "{}")])).await;
        }

        let events = prompt(&mut handle, "loop").await;

        assert_eq!(events.last(), Some(&end(StopReason::MaxTurnRequests)));
        assert_eq!(server.received_requests().await.unwrap().len(), 2);
    }

    #[tokio::test]
    async fn failed_turn_leaves_no_partial_messages_in_history() {
        let (server, _dir, mut handle) = session_with_notes(25).await;
        respond(&server, tool_calls(&[("c", "list_dir", "{}")])).await;
        respond(&server, crate::test_support::client_error()).await;
        respond(&server, reply(&["ok"])).await;

        let events = prompt(&mut handle, "first").await;
        assert!(matches!(events[2], Event::Error { .. }), "{events:?}");
        prompt(&mut handle, "second").await;

        assert_eq!(
            sent_messages(&server).await[2],
            json!([{"role": "user", "content": "second"}])
        );
    }
}
