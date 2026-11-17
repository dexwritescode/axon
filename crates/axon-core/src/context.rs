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
    lm_studio(http, root, api_key, model).await
}

async fn lm_studio(http: &Client, root: &str, api_key: &str, model: &str) -> Option<u64> {
    let url = format!("{root}/api/v0/models/{model}");

    let response = http.get(&url).bearer_auth(api_key).send().await.ok()?;

    let json: serde_json::Value = response.json().await.ok()?;
    json["loaded_context_length"].as_u64()
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
}
