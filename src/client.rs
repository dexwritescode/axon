#![allow(dead_code)] // wired up in axon-ptq (inference task)

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
        let chunk = chunk?;
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

    // Emit assembled tool calls in index order, then signal completion.
    let mut calls: Vec<_> = pending.into_iter().collect();
    calls.sort_by_key(|(idx, _)| *idx);
    for (_, (_, name, args_str)) in calls {
        let args = serde_json::from_str(&args_str).unwrap_or(serde_json::Value::Null);
        tx.send(Ok(AppEvent::ToolCall { name, args })).await?;
    }

    tx.send(Ok(AppEvent::Done)).await?;
    Ok(())
}
