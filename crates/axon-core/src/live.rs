//! Tests against a real inference server named by AXON_LIVE_BASE_URL and AXON_LIVE_MODEL, run by scripts/backend-test.sh.

use std::time::Duration;

use async_openai::types::chat::ChatCompletionRequestUserMessage;
use futures::StreamExt;

use crate::context;
use crate::model::{self, Delta};

#[tokio::test]
#[ignore = "needs a running server; see scripts/backend-test.sh"]
async fn live_backend_reports_its_context_and_answers() {
    let base_url = std::env::var("AXON_LIVE_BASE_URL").expect("AXON_LIVE_BASE_URL is not set");
    let model = std::env::var("AXON_LIVE_MODEL").expect("AXON_LIVE_MODEL is not set");

    let context = context::discover(&reqwest::Client::new(), &base_url, "none", &model).await;
    assert!(
        context.is_some(),
        "discover returned None for model {model}"
    );
    eprintln!("context: {context:?}");

    let config = model::ModelConfig {
        base_url,
        api_key: "none".into(),
        model,
        request_pause: Duration::ZERO,
    };
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
            Delta::ToolCall(chunk) => {
                if let Some(function) = &chunk.function
                    && let Some(name) = &function.name
                {
                    tool_call_names.push(name.clone());
                }
            }
        }
    }

    eprintln!("reply: {reply:?}");
    eprintln!("tool_call_names: {tool_call_names:?}");
    assert!(
        !reply.trim().is_empty() || !tool_call_names.is_empty(),
        "expected either non-empty reply text or at least one tool call"
    );
}
