//! Axon's configuration, read from env vars. The only place config is read.

use std::env;
use std::path::PathBuf;
use std::time::Duration;

use axon_core::{Backend, ModelConfig};

/// Everything Axon's env vars configure.
#[derive(Debug)]
pub struct Config {
    pub model: ModelConfig,
    /// Where sessions are saved.
    pub sessions: PathBuf,
    /// How long a paste is before the TUI shows it as a token; None leaves
    /// the TUI's default.
    pub paste_token_chars: Option<usize>,
}

impl Config {
    /// Reads this process's env vars. An empty var counts as unset.
    pub fn from_env() -> Result<Self, ConfigError> {
        Self::read(|name| env::var(name).ok(), env::home_dir())
    }

    fn read(
        lookup: impl Fn(&str) -> Option<String>,
        home: Option<PathBuf>,
    ) -> Result<Self, ConfigError> {
        let lookup = |name: &str| lookup(name).filter(|value| !value.is_empty());
        Ok(Self {
            model: model(lookup)?,
            sessions: sessions_dir(lookup, home)?,
            paste_token_chars: paste_token_chars(lookup)?,
        })
    }
}

/// Why the configuration could not be read.
#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    /// The env var `var` holds a value that can't be used.
    #[error("{var}: {message}")]
    Invalid { var: &'static str, message: String },
    /// Neither `AXON_SESSIONS_DIR` nor a home folder says where sessions go.
    #[error("no home folder to save sessions in; set AXON_SESSIONS_DIR")]
    NoHome,
}

/// `AXON_SESSIONS_DIR`, or `.axon/sessions` in the home folder.
fn sessions_dir(
    lookup: impl Fn(&str) -> Option<String>,
    home: Option<PathBuf>,
) -> Result<PathBuf, ConfigError> {
    if let Some(dir) = lookup("AXON_SESSIONS_DIR") {
        return Ok(dir.into());
    }
    let home = home.ok_or(ConfigError::NoHome)?;
    Ok(home.join(".axon").join("sessions"))
}

/// Defaults to LM Studio's local server; env vars override each field.
fn model(lookup: impl Fn(&str) -> Option<String>) -> Result<ModelConfig, ConfigError> {
    let var = |name, default: &str| lookup(name).unwrap_or_else(|| default.into());
    let invalid = |var, message| ConfigError::Invalid { var, message };
    let pause = var("AXON_REQUEST_PAUSE_SECS", "0");
    let pause = pause.parse().map_err(|_| {
        invalid(
            "AXON_REQUEST_PAUSE_SECS",
            format!("not a whole number of seconds: {pause}"),
        )
    })?;
    let backend: Backend = var("AXON_BACKEND", "lmstudio")
        .parse()
        .map_err(|e| invalid("AXON_BACKEND", format!("{e}")))?;
    let max_tokens = var("AXON_MAX_TOKENS", "16384");
    let max_tokens = max_tokens.parse().map_err(|_| {
        invalid(
            "AXON_MAX_TOKENS",
            format!("not a whole number: {max_tokens}"),
        )
    })?;
    Ok(ModelConfig {
        backend,
        base_url: var("OPENAI_BASE_URL", "http://localhost:1234/v1"),
        api_key: var("OPENAI_API_KEY", "lm-studio"),
        model: var("AXON_MODEL", "qwen/qwen3.6-35b-a3b"),
        request_pause: Duration::from_secs(pause),
        max_tokens,
    })
}

/// `AXON_PASTE_TOKEN_CHARS`, when set.
fn paste_token_chars(
    lookup: impl Fn(&str) -> Option<String>,
) -> Result<Option<usize>, ConfigError> {
    lookup("AXON_PASTE_TOKEN_CHARS")
        .map(|chars| {
            chars.parse().map_err(|_| ConfigError::Invalid {
                var: "AXON_PASTE_TOKEN_CHARS",
                message: format!("not a whole number: {chars}"),
            })
        })
        .transpose()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sessions_are_saved_in_the_home_folder_unless_the_env_var_says_where() {
        let home = Some(PathBuf::from("/home/u"));
        assert_eq!(
            sessions_dir(|_| None, home.clone()).unwrap(),
            PathBuf::from("/home/u/.axon/sessions")
        );
        let set = |name: &str| (name == "AXON_SESSIONS_DIR").then(|| "/elsewhere".into());
        assert_eq!(
            sessions_dir(set, home).unwrap(),
            PathBuf::from("/elsewhere")
        );
        assert_eq!(
            sessions_dir(|_| None, None).unwrap_err().to_string(),
            "no home folder to save sessions in; set AXON_SESSIONS_DIR"
        );
    }

    #[test]
    fn model_config_defaults_to_lm_studio() {
        let config = model(|_| None).unwrap();

        assert_eq!(config.backend, Backend::LmStudio);
        assert_eq!(config.base_url, "http://localhost:1234/v1");
        assert_eq!(config.api_key, "lm-studio");
        assert_eq!(config.model, "qwen/qwen3.6-35b-a3b");
        assert_eq!(config.request_pause, Duration::ZERO);
        assert_eq!(config.max_tokens, 16384);
    }

    #[test]
    fn env_vars_override_each_field() {
        let config = model(|name| match name {
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
        let err = model(|name| (name == "AXON_BACKEND").then(|| "lm-studio".into()));

        let message = err.unwrap_err().to_string();
        assert!(
            message.contains("AXON_BACKEND") && message.contains("lmstudio"),
            "{message}"
        );
    }

    #[test]
    fn a_bad_token_limit_is_an_error() {
        let err = model(|name| (name == "AXON_MAX_TOKENS").then(|| "lots".into()));

        assert_eq!(
            err.unwrap_err().to_string(),
            "AXON_MAX_TOKENS: not a whole number: lots"
        );
    }

    #[test]
    fn a_bad_pause_is_an_error() {
        let err = model(|name| (name == "AXON_REQUEST_PAUSE_SECS").then(|| "1.5".into()));

        assert_eq!(
            err.unwrap_err().to_string(),
            "AXON_REQUEST_PAUSE_SECS: not a whole number of seconds: 1.5"
        );
    }

    #[test]
    fn the_paste_token_size_is_read_when_set() {
        assert_eq!(paste_token_chars(|_| None).unwrap(), None);
        let set = |name: &str| (name == "AXON_PASTE_TOKEN_CHARS").then(|| "50".into());
        assert_eq!(paste_token_chars(set).unwrap(), Some(50));
        let bad = |name: &str| (name == "AXON_PASTE_TOKEN_CHARS").then(|| "lots".into());
        assert_eq!(
            paste_token_chars(bad).unwrap_err().to_string(),
            "AXON_PASTE_TOKEN_CHARS: not a whole number: lots"
        );
    }

    #[test]
    fn an_empty_var_counts_as_unset() {
        let empty = |_: &str| Some(String::new());
        let config = Config::read(empty, Some(PathBuf::from("/home/u"))).unwrap();

        assert_eq!(config.model.model, "qwen/qwen3.6-35b-a3b");
        assert_eq!(config.model.max_tokens, 16384);
        assert_eq!(config.sessions, PathBuf::from("/home/u/.axon/sessions"));
        assert_eq!(config.paste_token_chars, None);
    }
}
