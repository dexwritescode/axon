use std::collections::HashMap;

use anyhow::Result;
use async_openai::{
    Client,
    config::OpenAIConfig,
    types::chat::{
        ChatCompletionRequestMessage, ChatCompletionTools, CreateChatCompletionRequestArgs,
    },
};
use futures::StreamExt;
use tokio_stream::wrappers::ReceiverStream;

use crate::{config::BackendConfig, event::AppEvent};

#[derive(Clone)]
pub struct AxonClient {
    inner: Client<OpenAIConfig>,
    model: String,
}

impl AxonClient {
    pub fn new(config: &BackendConfig) -> Self {
        let openai_config = OpenAIConfig::new()
            .with_api_base(&config.base_url)
            .with_api_key(&config.api_key);
        Self {
            inner: Client::with_config(openai_config),
            model: config.model.clone(),
        }
    }

    /// Stream a chat completion, mapping SSE chunks to AppEvent variants.
    ///
    /// Token deltas are forwarded immediately. Tool call argument fragments are
    /// accumulated across chunks and emitted as ToolCall events once the stream
    /// ends, followed by Done.
    pub fn stream_chat(
        &self,
        messages: Vec<ChatCompletionRequestMessage>,
        tools: Vec<ChatCompletionTools>,
    ) -> impl futures::Stream<Item = Result<AppEvent>> {
        let client = self.inner.clone();
        let model = self.model.clone();
        let (tx, rx) = tokio::sync::mpsc::channel(64);

        tokio::spawn(async move {
            if let Err(e) = run_stream(client, model, messages, tools, &tx).await {
                let _ = tx.send(Err(e)).await;
            }
        });

        ReceiverStream::new(rx)
    }

    /// List model IDs available on the configured backend.
    pub async fn list_models(&self) -> Result<Vec<String>> {
        let resp = self.inner.models().list().await?;
        Ok(resp.data.into_iter().map(|m| m.id).collect())
    }
}

async fn run_stream(
    client: Client<OpenAIConfig>,
    model: String,
    messages: Vec<ChatCompletionRequestMessage>,
    tools: Vec<ChatCompletionTools>,
    tx: &tokio::sync::mpsc::Sender<Result<AppEvent>>,
) -> Result<()> {
    let mut req = CreateChatCompletionRequestArgs::default();
    req.model(&model).messages(messages).stream(true);
    if !tools.is_empty() {
        req.tools(tools);
    }

    let mut stream = client.chat().create_stream(req.build()?).await?;

    // index → (id, name, accumulated_args)
    let mut pending: HashMap<u32, (String, String, String)> = HashMap::new();

    while let Some(chunk) = stream.next().await {
        // async-openai emits JSONDeserialize("[DONE]") for the SSE sentinel — treat as end-of-stream
        let chunk = match chunk {
            Ok(c) => c,
            Err(async_openai::error::OpenAIError::JSONDeserialize(_, ref s)) if s == "[DONE]" => {
                break;
            }
            Err(e) => return Err(e.into()),
        };
        for choice in chunk.choices {
            if let Some(content) = choice.delta.content
                && !content.is_empty()
            {
                tx.send(Ok(AppEvent::Token(content))).await?;
            }
            if let Some(tcs) = choice.delta.tool_calls {
                for tc in tcs {
                    let entry = pending.entry(tc.index).or_default();
                    if let Some(id) = tc.id {
                        entry.0 = id;
                    }
                    if let Some(func) = tc.function {
                        if let Some(name) = func.name {
                            entry.1 = name;
                        }
                        if let Some(args) = func.arguments {
                            entry.2.push_str(&args);
                        }
                    }
                }
            }
        }
    }

    for event in assemble_tool_calls(pending) {
        tx.send(Ok(event)).await?;
    }
    tx.send(Ok(AppEvent::Done)).await?;
    Ok(())
}

// index → (id, name, accumulated_args)
type PendingToolCalls = HashMap<u32, (String, String, String)>;

fn assemble_tool_calls(pending: PendingToolCalls) -> Vec<AppEvent> {
    let mut calls: Vec<_> = pending.into_iter().collect();
    calls.sort_by_key(|(idx, _)| *idx);
    calls
        .into_iter()
        .map(|(_, (_, name, args_str))| {
            let args = serde_json::from_str(&args_str).unwrap_or(serde_json::Value::Null);
            AppEvent::ToolCall { name, args }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn pending(entries: &[(u32, &str, &str, &str)]) -> PendingToolCalls {
        entries
            .iter()
            .map(|(idx, id, name, args)| {
                (*idx, (id.to_string(), name.to_string(), args.to_string()))
            })
            .collect()
    }

    #[test]
    fn single_tool_call_valid_json() {
        let events = assemble_tool_calls(pending(&[(
            0,
            "call_abc",
            "read_file",
            r#"{"path":"/etc/hostname"}"#,
        )]));
        assert_eq!(
            events,
            vec![AppEvent::ToolCall {
                name: "read_file".into(),
                args: json!({"path": "/etc/hostname"}),
            }]
        );
    }

    #[test]
    fn multiple_tool_calls_emitted_in_index_order() {
        // Insert in reverse order to verify sorting.
        let events = assemble_tool_calls(pending(&[
            (1, "call_2", "shell", r#"{"cmd":"ls"}"#),
            (0, "call_1", "read_file", r#"{"path":"/etc/hostname"}"#),
        ]));
        assert_eq!(
            events[0],
            AppEvent::ToolCall {
                name: "read_file".into(),
                args: json!({"path": "/etc/hostname"})
            }
        );
        assert_eq!(
            events[1],
            AppEvent::ToolCall {
                name: "shell".into(),
                args: json!({"cmd": "ls"})
            }
        );
    }

    #[test]
    fn invalid_json_args_become_null() {
        let events = assemble_tool_calls(pending(&[(0, "call_x", "read_file", "not json")]));
        assert_eq!(
            events,
            vec![AppEvent::ToolCall {
                name: "read_file".into(),
                args: serde_json::Value::Null,
            }]
        );
    }

    #[test]
    fn empty_args_string_becomes_null() {
        let events = assemble_tool_calls(pending(&[(0, "call_x", "read_file", "")]));
        assert_eq!(
            events,
            vec![AppEvent::ToolCall {
                name: "read_file".into(),
                args: serde_json::Value::Null,
            }]
        );
    }

    #[test]
    fn no_pending_tool_calls_yields_empty() {
        assert!(assemble_tool_calls(HashMap::new()).is_empty());
    }

    #[test]
    fn fragmented_args_assemble_correctly() {
        // Simulate the three-chunk arg stream: '{"pa' + 'th":"' + '/etc/hostname"}'
        let mut p: PendingToolCalls = HashMap::new();
        let entry = p.entry(0).or_default();
        entry.0 = "call_1".into();
        entry.1 = "read_file".into();
        entry.2.push_str(r#"{"pa"#);
        entry.2.push_str(r#"th":""#);
        entry.2.push_str(r#"/etc/hostname"}"#);

        let events = assemble_tool_calls(p);
        assert_eq!(
            events,
            vec![AppEvent::ToolCall {
                name: "read_file".into(),
                args: json!({"path": "/etc/hostname"}),
            }]
        );
    }
}
