//! Tests against a real inference server named by AXON_LIVE_BACKEND, AXON_LIVE_BASE_URL and AXON_LIVE_MODEL, run by scripts/backend-test.sh.

use std::time::Duration;

use async_openai::types::chat::ChatCompletionRequestUserMessage;
use futures::StreamExt;

use axon_protocol::{Event, Resume, SessionFactory, StopReason};

use crate::backend::Backend;
use crate::context;
use crate::model::{self, Delta, RequestPause};
use crate::test_support::{end, prompt};
use crate::{AgentConfig, CoreFactory};

/// The server the AXON_LIVE_* env vars name.
fn live_config() -> model::ModelConfig {
    let base_url = std::env::var("AXON_LIVE_BASE_URL").expect("AXON_LIVE_BASE_URL is not set");
    let model = std::env::var("AXON_LIVE_MODEL").expect("AXON_LIVE_MODEL is not set");
    let backend: Backend = std::env::var("AXON_LIVE_BACKEND")
        .expect("AXON_LIVE_BACKEND is not set")
        .parse()
        .unwrap();
    model::ModelConfig {
        backend,
        base_url,
        api_key: "none".into(),
        model,
        request_pause: RequestPause::Fixed(Duration::ZERO),
        max_tokens: 4096,
        reasoning_effort: None,
    }
}

#[tokio::test]
#[ignore = "needs a running server; see scripts/backend-test.sh"]
async fn live_backend_reports_its_context_and_answers() {
    let config = live_config();
    let (backend, base_url, model) = (config.backend, &config.base_url, &config.model);

    let context =
        context::discover(&reqwest::Client::new(), backend, base_url, "none", model).await;
    assert!(
        context.is_some(),
        "discover returned None for model {model}"
    );
    eprintln!("context: {context:?}");

    let client = model::ModelClient::new(&config);

    let messages = vec![ChatCompletionRequestUserMessage::from("Reply with one word: ok").into()];

    let mut stream = client
        .stream_chat(messages, crate::tools::schemas())
        .await
        .unwrap();

    let mut reply = String::new();
    let mut tool_call_names: Vec<String> = Vec::new();
    while let Some(delta) = stream.next().await {
        let delta = delta.unwrap();
        match delta {
            Delta::Text(text) => reply.push_str(&text),
            Delta::Reasoning(_) => {}
            Delta::ToolCall(chunk) => {
                if let Some(function) = &chunk.function
                    && let Some(name) = &function.name
                {
                    tool_call_names.push(name.clone());
                }
            }
            Delta::Truncated => panic!("the reply hit the token limit"),
        }
    }

    eprintln!("reply: {reply:?}");
    eprintln!("tool_call_names: {tool_call_names:?}");
    assert!(
        !reply.trim().is_empty() || !tool_call_names.is_empty(),
        "expected either non-empty reply text or at least one tool call"
    );
}

#[tokio::test]
#[ignore = "needs a running server; see scripts/backend-test.sh"]
async fn live_profile_prompt_reaches_the_model_with_no_tools() {
    let root = tempfile::tempdir().unwrap();
    let agent = AgentConfig {
        system_prompt: Some("Your name is BUDDY. You have no tools.".into()),
        tools: Some(vec![]),
    };
    let factory = CoreFactory::new(live_config(), agent, root.path().into());
    let mut handle = factory
        .new_session(root.path().into(), Resume::New)
        .unwrap();
    handle.events.recv().await;

    let events = prompt(&mut handle, "What is your name? Answer with the name only.").await;

    let reply: String = events
        .iter()
        .filter_map(|event| match event {
            Event::TextDelta { text } => Some(text.as_str()),
            _ => None,
        })
        .collect();
    eprintln!("reply: {reply:?}");
    assert_eq!(events.last(), Some(&end(StopReason::EndTurn)), "{events:?}");
    assert!(reply.to_lowercase().contains("buddy"), "{reply}");
}
