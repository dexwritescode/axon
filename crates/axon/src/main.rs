use std::env;

use anyhow::{Result, bail};
use axon_core::{CoreFactory, ModelConfig};

#[tokio::main]
async fn main() -> Result<()> {
    match env::args().nth(1).as_deref() {
        None => axon_tui::run(&CoreFactory::new(model_config())).await?,
        Some("acp") => axon_acp::serve().await?,
        Some(other) => bail!("unknown command: {other}"),
    }
    Ok(())
}

/// Defaults to LM Studio's local server; env vars override each field.
fn model_config() -> ModelConfig {
    let var = |name, default: &str| env::var(name).unwrap_or_else(|_| default.into());
    ModelConfig {
        base_url: var("OPENAI_BASE_URL", "http://localhost:1234/v1"),
        api_key: var("OPENAI_API_KEY", "lm-studio"),
        model: var("AXON_MODEL", "qwen/qwen3.6-35b-a3b"),
    }
}
