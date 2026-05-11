//! Live integration tests against a real neurons-service instance.
//! Skipped automatically when AXON_TEST_BASE_URL is not set.

use async_openai::types::chat::{
    ChatCompletionRequestMessage, ChatCompletionRequestUserMessageArgs,
};
use axon::{client::AxonClient, config::BackendConfig, event::AppEvent};
use futures::StreamExt;

fn live_client() -> Option<AxonClient> {
    let base_url = std::env::var("AXON_TEST_BASE_URL").ok()?;
    Some(AxonClient::new(&BackendConfig {
        base_url,
        api_key: std::env::var("OPENAI_API_KEY").unwrap_or_else(|_| "x".to_string()),
        model: std::env::var("AXON_TEST_MODEL")
            .unwrap_or_else(|_| "mlx-community/TinyLlama-1.1B-Chat-v1.0-4bit".to_string()),
    }))
}

#[tokio::test]
async fn list_models_returns_at_least_one() {
    let Some(client) = live_client() else {
        return;
    };
    let models = client.list_models().await.expect("list_models failed");
    assert!(
        !models.is_empty(),
        "expected at least one model from the server"
    );
}

#[tokio::test]
async fn stream_chat_yields_tokens_then_done() {
    let Some(client) = live_client() else {
        return;
    };

    let msg = ChatCompletionRequestUserMessageArgs::default()
        .content("Reply with one word only.")
        .build()
        .unwrap();
    let messages = vec![ChatCompletionRequestMessage::User(msg)];

    let mut stream = client.stream_chat(messages, vec![]);
    let mut got_token = false;
    let mut got_done = false;

    while let Some(result) = stream.next().await {
        match result.expect("stream error") {
            AppEvent::Token(_) => got_token = true,
            AppEvent::Done => {
                got_done = true;
                break;
            }
            _ => {}
        }
    }

    assert!(got_token, "expected at least one Token event");
    assert!(got_done, "expected Done event");
}
