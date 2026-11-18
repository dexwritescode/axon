use std::env;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use axon_core::{CoreFactory, ModelConfig};

#[tokio::main]
async fn main() -> Result<()> {
    match env::args().nth(1).as_deref() {
        None => {
            let config = model_config(|name| env::var(name).ok())?;
            axon_tui::run(&CoreFactory::new(config)).await?
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
    Ok(ModelConfig {
        base_url: var("OPENAI_BASE_URL", "http://localhost:1234/v1"),
        api_key: var("OPENAI_API_KEY", "lm-studio"),
        model: var("AXON_MODEL", "qwen/qwen3.6-35b-a3b"),
        request_pause: Duration::from_secs(pause),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn model_config_defaults_to_lm_studio() {
        let config = model_config(|_| None).unwrap();

        assert_eq!(config.base_url, "http://localhost:1234/v1");
        assert_eq!(config.api_key, "lm-studio");
        assert_eq!(config.model, "qwen/qwen3.6-35b-a3b");
        assert_eq!(config.request_pause, Duration::ZERO);
    }

    #[test]
    fn env_vars_override_each_field() {
        let config = model_config(|name| match name {
            "AXON_REQUEST_PAUSE_SECS" => Some("30".into()),
            _ => Some(format!("{name}-value")),
        })
        .unwrap();

        assert_eq!(config.base_url, "OPENAI_BASE_URL-value");
        assert_eq!(config.api_key, "OPENAI_API_KEY-value");
        assert_eq!(config.model, "AXON_MODEL-value");
        assert_eq!(config.request_pause, Duration::from_secs(30));
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
}
