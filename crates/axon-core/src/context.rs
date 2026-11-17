//! Discover the model's loaded context size from the inference server.

use reqwest::Client;

/// Returns the first answer from a list of probes, or None when no probe answers.
#[cfg_attr(
    not(test),
    expect(dead_code, reason = "the context warning will call it")
)]
pub(crate) async fn discover(
    http: &Client,
    base_url: &str,
    api_key: &str,
    model: &str,
) -> Option<u64> {
    let root = base_url
        .strip_suffix("/v1")
        .or_else(|| base_url.strip_suffix("/v1/"))
        .unwrap_or(base_url);
    if let Some(n) = lm_studio(http, root, api_key, model).await {
        return Some(n);
    }
    if let Some(n) = vllm(http, base_url, api_key, model).await {
        return Some(n);
    }
    llama_cpp(http, root, api_key).await
}

async fn lm_studio(http: &Client, root: &str, api_key: &str, model: &str) -> Option<u64> {
    let url = format!("{root}/api/v0/models/{model}");

    let response = http.get(&url).bearer_auth(api_key).send().await.ok()?;

    let json: serde_json::Value = response.json().await.ok()?;
    json["loaded_context_length"].as_u64()
}

async fn vllm(http: &Client, base_url: &str, api_key: &str, model: &str) -> Option<u64> {
    let url = format!("{base_url}/models");

    let response = http.get(&url).bearer_auth(api_key).send().await.ok()?;

    let json: serde_json::Value = response.json().await.ok()?;
    json["data"]
        .as_array()?
        .iter()
        .find(|m| m["id"].as_str() == Some(model))?["max_model_len"]
        .as_u64()
}

async fn llama_cpp(http: &Client, root: &str, api_key: &str) -> Option<u64> {
    let url = format!("{root}/props");

    let response = http.get(&url).bearer_auth(api_key).send().await.ok()?;

    let json: serde_json::Value = response.json().await.ok()?;
    json["default_generation_settings"]["n_ctx"].as_u64()
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
}
