use std::env;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use axon_core::{Backend, CoreFactory, ModelConfig};

#[tokio::main]
async fn main() -> Result<()> {
    match env::args().nth(1).as_deref() {
        None => {
            let lookup = |name: &str| env::var(name).ok();
            let config = model_config(lookup)?;
            axon_tui::run(&CoreFactory::new(config), tui_settings(lookup)?).await?
        }
        Some("acp") => axon_acp::serve().await?,
        Some(other) => bail!("unknown command: {other}"),
    }
    Ok(())
}

/// Defaults to LM Studio's local server; env vars override each field.
fn model_config(lookup: impl Fn(&str) -> Option<String>) -> Result<ModelConfig> {
    let var = |name, default: &str| lookup(name).unwrap_or_else(|| default.into());
    let pause = var("AXON_REQUEST_PAUSE_SECS", "0");
    let pause = pause.parse().with_context(|| {
        format!("AXON_REQUEST_PAUSE_SECS: not a whole number of seconds: {pause}")
    })?;
    let backend: Backend = var("AXON_BACKEND", "lmstudio")
        .parse()
        .context("AXON_BACKEND")?;
    let max_tokens = var("AXON_MAX_TOKENS", "16384");
    let max_tokens = max_tokens
        .parse()
        .with_context(|| format!("AXON_MAX_TOKENS: not a whole number: {max_tokens}"))?;
    Ok(ModelConfig {
        backend,
        base_url: var("OPENAI_BASE_URL", "http://localhost:1234/v1"),
        api_key: var("OPENAI_API_KEY", "lm-studio"),
        model: var("AXON_MODEL", "qwen/qwen3.6-35b-a3b"),
        request_pause: Duration::from_secs(pause),
        max_tokens,
    })
}

fn tui_settings(lookup: impl Fn(&str) -> Option<String>) -> Result<axon_tui::Settings> {
    let mut settings = axon_tui::Settings::default();
    if let Some(chars) = lookup("AXON_PASTE_TOKEN_CHARS") {
        settings.paste_token_chars = chars
            .parse()
            .with_context(|| format!("AXON_PASTE_TOKEN_CHARS: not a whole number: {chars}"))?;
    }
    Ok(settings)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn model_config_defaults_to_lm_studio() {
        let config = model_config(|_| None).unwrap();

        assert_eq!(config.backend, Backend::LmStudio);
        assert_eq!(config.base_url, "http://localhost:1234/v1");
        assert_eq!(config.api_key, "lm-studio");
        assert_eq!(config.model, "qwen/qwen3.6-35b-a3b");
        assert_eq!(config.request_pause, Duration::ZERO);
        assert_eq!(config.max_tokens, 16384);
    }

    #[test]
    fn env_vars_override_each_field() {
        let config = model_config(|name| match name {
            "AXON_REQUEST_PAUSE_SECS" => Some("30".into()),
            "AXON_MAX_TOKENS" => Some("2048".into()),
            "AXON_BACKEND" => Some("ollama".into()),
            _ => Some(format!("{name}-value")),
        })
        .unwrap();

        assert_eq!(config.base_url, "OPENAI_BASE_URL-value");
        assert_eq!(config.api_key, "OPENAI_API_KEY-value");
        assert_eq!(config.model, "AXON_MODEL-value");
        assert_eq!(config.request_pause, Duration::from_secs(30));
        assert_eq!(config.max_tokens, 2048);
        assert_eq!(config.backend, Backend::Ollama);
    }

    #[test]
    fn an_unknown_backend_is_an_error() {
        let err = model_config(|name| (name == "AXON_BACKEND").then(|| "lm-studio".into()));

        let message = format!("{:#}", err.unwrap_err());
        assert!(
            message.contains("AXON_BACKEND") && message.contains("lmstudio"),
            "{message}"
        );
    }

    #[test]
    fn a_bad_token_limit_is_an_error() {
        let err = model_config(|name| (name == "AXON_MAX_TOKENS").then(|| "lots".into()));

        assert!(err.unwrap_err().to_string().contains("AXON_MAX_TOKENS"));
    }

    #[test]
    fn a_bad_pause_is_an_error() {
        let err = model_config(|name| (name == "AXON_REQUEST_PAUSE_SECS").then(|| "1.5".into()));

        assert!(
            err.unwrap_err()
                .to_string()
                .contains("AXON_REQUEST_PAUSE_SECS")
        );
    }

    #[test]
    fn tui_settings_read_the_paste_token_size() {
        assert_eq!(tui_settings(|_| None).unwrap().paste_token_chars, 1000);
        let set = |name: &str| (name == "AXON_PASTE_TOKEN_CHARS").then(|| "50".into());
        assert_eq!(tui_settings(set).unwrap().paste_token_chars, 50);
        let bad = |name: &str| (name == "AXON_PASTE_TOKEN_CHARS").then(|| "lots".into());
        assert!(tui_settings(bad).is_err());
    }
}
