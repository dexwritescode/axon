/// Integration tests against a running Prism mock server.
///
/// Prism loads the official OpenAI OpenAPI spec and validates every request
/// against the schema. A passing test proves AxonClient sends a spec-compliant
/// request AND can parse the spec-compliant response.
///
/// Set PRISM_BASE_URL (e.g. http://127.0.0.1:4010/v1) to run these tests.
/// They are silently skipped when the variable is absent.
use axon::client::AxonClient;
use axon::config::BackendConfig;

fn prism_config() -> Option<BackendConfig> {
    let base_url = std::env::var("PRISM_BASE_URL").ok()?;
    Some(BackendConfig {
        base_url,
        api_key: "test".to_string(),
        model: String::new(),
    })
}

#[tokio::test]
async fn list_models_contract() {
    let Some(config) = prism_config() else {
        return;
    };
    let client = AxonClient::new(&config);
    // Prism validates the request against the OpenAI spec.
    // A successful response confirms the request shape and response parsing are both correct.
    let models = client
        .list_models()
        .await
        .expect("list_models failed against Prism");
    assert!(
        models.iter().all(|m| !m.is_empty()),
        "Prism returned model IDs that were empty strings"
    );
}
