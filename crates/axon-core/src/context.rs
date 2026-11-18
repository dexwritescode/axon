//! Discover the model's loaded context size from the inference server.

use reqwest::Client;
use serde_json::Value;

/// Asks LM Studio, vLLM, llama.cpp and Ollama at once; the first of them, in
/// that order, to report a size wins. None when no server answers.
pub(crate) async fn discover(
    http: &Client,
    base_url: &str,
    api_key: &str,
    model: &str,
) -> Option<u64> {
    let base_url = base_url.trim_end_matches('/');
    let root = base_url.strip_suffix("/v1").unwrap_or(base_url);
    let get = |url: String| get_json(http, url, api_key);
    let (lm_studio, vllm, llama_cpp, ollama) = tokio::join!(
        get(format!("{root}/api/v0/models/{model}")),
        get(format!("{base_url}/models")),
        get(format!("{root}/props")),
        get(format!("{root}/api/ps")),
    );
    lm_studio
        .and_then(|json| json["loaded_context_length"].as_u64())
        .or_else(|| {
            vllm.and_then(|json| {
                find_model(&json["data"], &["id"], model)?["max_model_len"].as_u64()
            })
        })
        .or_else(|| {
            llama_cpp.and_then(|json| json["default_generation_settings"]["n_ctx"].as_u64())
        })
        .or_else(|| {
            ollama.and_then(|json| {
                find_model(&json["models"], &["name", "model"], model)?["context_length"].as_u64()
            })
        })
}

async fn get_json(http: &Client, url: String, api_key: &str) -> Option<Value> {
    let response = http.get(url).bearer_auth(api_key).send().await.ok()?;
    response.error_for_status().ok()?.json().await.ok()
}

/// The entry in a list of models whose `keys` field names `model`.
fn find_model<'a>(list: &'a Value, keys: &[&str], model: &str) -> Option<&'a Value> {
    list.as_array()?
        .iter()
        .find(|entry| keys.iter().any(|key| entry[*key].as_str() == Some(model)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::api_url;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    #[tokio::test]
    async fn lm_studio_returns_loaded_context_length() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/v0/models/test-model"))
            .respond_with(
                ResponseTemplate::new(200).set_body_string(r#"{"loaded_context_length": 128000}"#),
            )
            .mount(&server)
            .await;

        let http = Client::new();
        let result = discover(&http, &api_url(&server), "test", "test-model").await;
        assert_eq!(result, Some(128000));
    }

    #[tokio::test]
    async fn a_base_url_with_a_trailing_slash_finds_the_same_paths() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/v1/models"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_string(r#"{"data":[{"id":"test-model","max_model_len":32768}]}"#),
            )
            .mount(&server)
            .await;

        let http = Client::new();
        let result = discover(
            &http,
            &format!("{}/", api_url(&server)),
            "test",
            "test-model",
        )
        .await;
        assert_eq!(result, Some(32768));
    }

    #[tokio::test]
    async fn an_error_status_is_no_answer() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/v0/models/test-model"))
            .respond_with(
                ResponseTemplate::new(500).set_body_string(r#"{"loaded_context_length": 128000}"#),
            )
            .mount(&server)
            .await;

        let http = Client::new();
        let result = discover(&http, &api_url(&server), "test", "test-model").await;
        assert_eq!(result, None);
    }

    #[tokio::test]
    async fn lm_studio_404_returns_none() {
        let server = MockServer::start().await;

        let http = Client::new();
        let result = discover(&http, &api_url(&server), "test", "test-model").await;
        assert_eq!(result, None);
    }

    #[tokio::test]
    async fn vllm_returns_max_model_len_for_matching_model() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/v1/models"))
            .respond_with(
                ResponseTemplate::new(200).set_body_string(
                    r#"{"data":[{"id":"other-model","max_model_len":4096},{"id":"test-model","max_model_len":32768}]}"#,
                ),
            )
            .mount(&server)
            .await;

        let http = Client::new();
        let result = discover(&http, &api_url(&server), "test", "test-model").await;
        assert_eq!(result, Some(32768));
    }

    #[tokio::test]
    async fn vllm_no_matching_model_returns_none() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/v1/models"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_string(r#"{"data":[{"id":"other-model","max_model_len":4096}]}"#),
            )
            .mount(&server)
            .await;

        let http = Client::new();
        let result = discover(&http, &api_url(&server), "test", "test-model").await;
        assert_eq!(result, None);
    }

    #[tokio::test]
    async fn llama_cpp_returns_n_ctx() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/props"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_string(r#"{"default_generation_settings":{"n_ctx":8192}}"#),
            )
            .mount(&server)
            .await;

        let http = Client::new();
        let result = discover(&http, &api_url(&server), "test", "test-model").await;
        assert_eq!(result, Some(8192));
    }

    #[tokio::test]
    async fn llama_cpp_no_n_ctx_returns_none() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/props"))
            .respond_with(
                ResponseTemplate::new(200).set_body_string(r#"{"default_generation_settings":{}}"#),
            )
            .mount(&server)
            .await;

        let http = Client::new();
        let result = discover(&http, &api_url(&server), "test", "test-model").await;
        assert_eq!(result, None);
    }

    #[tokio::test]
    async fn ollama_returns_context_length_for_matching_model() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/ps"))
            .respond_with(
                ResponseTemplate::new(200).set_body_string(
                    r#"{"models":[{"name":"other-model","model":"other-model","context_length":4096},{"name":"test-model","model":"test-model","context_length":32768}]}"#,
                ),
            )
            .mount(&server)
            .await;

        let http = Client::new();
        let result = discover(&http, &api_url(&server), "test", "test-model").await;
        assert_eq!(result, Some(32768));
    }

    #[tokio::test]
    async fn ollama_no_matching_model_returns_none() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/ps"))
            .respond_with(
                ResponseTemplate::new(200).set_body_string(
                    r#"{"models":[{"name":"other-model","model":"other-model","context_length":4096}]}"#,
                ),
            )
            .mount(&server)
            .await;

        let http = Client::new();
        let result = discover(&http, &api_url(&server), "test", "test-model").await;
        assert_eq!(result, None);
    }
}
