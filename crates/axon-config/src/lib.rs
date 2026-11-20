//! Axon's configuration, read from env vars. The only place config is read.

use std::env;
use std::fs;
use std::path::PathBuf;
use std::time::Duration;

use axon_core::{AgentConfig, Backend, ModelConfig, RequestPause};
use serde::Deserialize;

/// Everything Axon's env vars configure.
#[derive(Debug)]
pub struct Config {
    pub model: ModelConfig,
    /// What the profile in `AXON_PROFILE` sets; the default without one.
    pub agent: AgentConfig,
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
            agent: agent(lookup)?,
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
    /// The profile file at `path` can't be read or used.
    #[error("profile {path}: {message}")]
    Profile { path: PathBuf, message: String },
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
    let pause = Duration::from_secs(pause.parse().map_err(|_| {
        invalid(
            "AXON_REQUEST_PAUSE_SECS",
            format!("not a whole number of seconds: {pause}"),
        )
    })?);
    // With a ratio, the seconds cap the pause; 0 leaves it uncapped.
    let request_pause = match lookup("AXON_REQUEST_PAUSE_RATIO") {
        None => RequestPause::Fixed(pause),
        Some(ratio) => RequestPause::Ratio {
            ratio: ratio
                .parse()
                .ok()
                .filter(|r: &f64| r.is_finite() && *r >= 0.0)
                .ok_or_else(|| {
                    invalid(
                        "AXON_REQUEST_PAUSE_RATIO",
                        format!("not a number from 0 up: {ratio}"),
                    )
                })?,
            max: (!pause.is_zero()).then_some(pause),
        },
    };
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
    let reasoning_effort = lookup("AXON_REASONING_EFFORT")
        .map(|effort| effort.parse())
        .transpose()
        .map_err(|e| invalid("AXON_REASONING_EFFORT", format!("{e}")))?;
    Ok(ModelConfig {
        backend,
        base_url: var("OPENAI_BASE_URL", "http://localhost:1234/v1"),
        api_key: var("OPENAI_API_KEY", "lm-studio"),
        model: var("AXON_MODEL", "qwen/qwen3.6-35b-a3b"),
        request_pause,
        max_tokens,
        reasoning_effort,
    })
}

/// What a profile file holds. Unknown fields are errors, so a typo isn't
/// silently ignored.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Profile {
    system_prompt: Option<String>,
    tools: Option<Vec<String>>,
}

/// The profile file `AXON_PROFILE` names, or the default without one.
fn agent(lookup: impl Fn(&str) -> Option<String>) -> Result<AgentConfig, ConfigError> {
    let Some(path) = lookup("AXON_PROFILE") else {
        return Ok(AgentConfig::default());
    };
    let path = PathBuf::from(path);
    let error = |message| ConfigError::Profile {
        path: path.clone(),
        message,
    };
    let text = fs::read_to_string(&path).map_err(|e| error(e.to_string()))?;
    profile(&text).map_err(error)
}

/// Parse a profile, checking that it names only built-in tools.
fn profile(text: &str) -> Result<AgentConfig, String> {
    let profile: Profile = toml::from_str(text).map_err(|e| e.message().to_string())?;
    if let Some(tools) = &profile.tools {
        let known = axon_core::tool_names();
        if let Some(unknown) = tools.iter().find(|tool| !known.contains(tool)) {
            return Err(format!(
                "unknown tool {unknown}; the tools are {}",
                known.join(", ")
            ));
        }
    }
    Ok(AgentConfig {
        system_prompt: profile.system_prompt,
        tools: profile.tools,
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
    use axon_core::ReasoningEffort;

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
        assert_eq!(config.request_pause, RequestPause::Fixed(Duration::ZERO));
        assert_eq!(config.max_tokens, 16384);
        assert_eq!(config.reasoning_effort, None);
    }

    #[test]
    fn env_vars_override_each_field() {
        let config = model(|name| match name {
            "AXON_REQUEST_PAUSE_SECS" => Some("30".into()),
            "AXON_REQUEST_PAUSE_RATIO" => Some("0.5".into()),
            "AXON_MAX_TOKENS" => Some("2048".into()),
            "AXON_BACKEND" => Some("ollama".into()),
            "AXON_REASONING_EFFORT" => Some("none".into()),
            _ => Some(format!("{name}-value")),
        })
        .unwrap();

        assert_eq!(config.base_url, "OPENAI_BASE_URL-value");
        assert_eq!(config.api_key, "OPENAI_API_KEY-value");
        assert_eq!(config.model, "AXON_MODEL-value");
        assert_eq!(
            config.request_pause,
            RequestPause::Ratio {
                ratio: 0.5,
                max: Some(Duration::from_secs(30))
            }
        );
        assert_eq!(config.max_tokens, 2048);
        assert_eq!(config.backend, Backend::Ollama);
        assert_eq!(config.reasoning_effort, Some(ReasoningEffort::None));
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
    fn an_unknown_reasoning_effort_is_an_error() {
        let err = model(|name| (name == "AXON_REASONING_EFFORT").then(|| "off".into()));

        let message = err.unwrap_err().to_string();
        assert!(
            message.contains("AXON_REASONING_EFFORT") && message.contains("none, low"),
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
    fn a_pause_ratio_without_seconds_has_no_cap_and_a_bad_one_is_an_error() {
        let ratio = |value: &'static str| {
            model(move |name| (name == "AXON_REQUEST_PAUSE_RATIO").then(|| value.into()))
        };

        assert_eq!(
            ratio("2").unwrap().request_pause,
            RequestPause::Ratio {
                ratio: 2.0,
                max: None
            }
        );
        for bad in ["-1", "lots", "inf"] {
            assert_eq!(
                ratio(bad).unwrap_err().to_string(),
                format!("AXON_REQUEST_PAUSE_RATIO: not a number from 0 up: {bad}")
            );
        }
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
    fn a_profile_sets_the_system_prompt_and_tools() {
        let text = "system_prompt = \"\"\"\nYou are BUDDY.\nBe brief.\"\"\"\ntools = [\"read_file\", \"grep\"]";
        let config = profile(text).unwrap();

        assert_eq!(
            config.system_prompt.as_deref(),
            Some("You are BUDDY.\nBe brief.")
        );
        assert_eq!(config.tools, Some(vec!["read_file".into(), "grep".into()]));
    }

    #[test]
    fn a_profile_may_leave_fields_out_or_turn_every_tool_off() {
        assert_eq!(profile("").unwrap(), AgentConfig::default());
        assert_eq!(profile("tools = []").unwrap().tools, Some(vec![]));
    }

    #[test]
    fn an_unknown_field_or_tool_in_a_profile_is_an_error() {
        let typo = profile("sytem_prompt = 'hi'").unwrap_err();
        assert!(typo.contains("sytem_prompt"), "{typo}");

        let tool = profile("tools = ['rm']").unwrap_err();
        assert!(
            tool.starts_with("unknown tool rm; the tools are read_file,"),
            "{tool}"
        );

        let syntax = profile("tools = [").unwrap_err();
        assert!(!syntax.is_empty());
    }

    #[test]
    fn the_profile_is_read_from_the_file_axon_profile_names() {
        assert_eq!(agent(|_| None).unwrap(), AgentConfig::default());

        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("buddy.toml");
        fs::write(&file, "tools = []").unwrap();
        let path = file.to_str().unwrap().to_string();
        let set = |name: &str| (name == "AXON_PROFILE").then(|| path.clone());
        assert_eq!(agent(set).unwrap().tools, Some(vec![]));

        fs::write(&file, "tools = ['rm']").unwrap();
        let message = agent(set).unwrap_err().to_string();
        assert!(
            message.starts_with(&format!("profile {path}: unknown tool rm")),
            "{message}"
        );
    }

    #[test]
    fn a_missing_profile_file_is_an_error() {
        let missing = |name: &str| (name == "AXON_PROFILE").then(|| "/no/such/profile.toml".into());

        let message = agent(missing).unwrap_err().to_string();
        assert!(
            message.starts_with("profile /no/such/profile.toml: "),
            "{message}"
        );
    }

    #[test]
    fn an_empty_var_counts_as_unset() {
        let empty = |_: &str| Some(String::new());
        let config = Config::read(empty, Some(PathBuf::from("/home/u"))).unwrap();

        assert_eq!(config.model.model, "qwen/qwen3.6-35b-a3b");
        assert_eq!(config.model.max_tokens, 16384);
        assert_eq!(config.model.reasoning_effort, None);
        assert_eq!(config.sessions, PathBuf::from("/home/u/.axon/sessions"));
        assert_eq!(config.paste_token_chars, None);
        assert_eq!(config.agent, AgentConfig::default());
    }
}
