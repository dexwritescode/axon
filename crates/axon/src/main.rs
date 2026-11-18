use std::env;
use std::path::PathBuf;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use axon_core::{Backend, CoreFactory, ModelConfig};
use axon_protocol::Resume;

/// What the command line asks for.
#[derive(Debug, PartialEq)]
enum Cli {
    Tui(Resume),
    Acp,
}

#[tokio::main]
async fn main() -> Result<()> {
    let args: Vec<String> = env::args().skip(1).collect();
    match cli(&args)? {
        Cli::Tui(resume) => {
            let lookup = |name: &str| env::var(name).ok();
            let config = model_config(lookup)?;
            let sessions = sessions_dir(lookup, env::home_dir())?;
            let factory = CoreFactory::new(config, sessions);
            axon_tui::run(&factory, tui_settings(lookup)?, resume).await?
        }
        Cli::Acp => axon_acp::serve().await?,
    }
    Ok(())
}

fn cli(args: &[String]) -> Result<Cli> {
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    Ok(match args[..] {
        [] => Cli::Tui(Resume::New),
        ["--continue"] => Cli::Tui(Resume::Latest),
        ["--resume", id] => Cli::Tui(Resume::Id(id.into())),
        ["--resume"] => bail!("--resume needs a session id"),
        ["acp"] => Cli::Acp,
        _ => bail!("unknown command: {}", args.join(" ")),
    })
}

/// `AXON_SESSIONS_DIR`, or `.axon/sessions` in the home folder.
fn sessions_dir(lookup: impl Fn(&str) -> Option<String>, home: Option<PathBuf>) -> Result<PathBuf> {
    if let Some(dir) = lookup("AXON_SESSIONS_DIR") {
        return Ok(dir.into());
    }
    let home = home.context("no home folder to save sessions in; set AXON_SESSIONS_DIR")?;
    Ok(home.join(".axon").join("sessions"))
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

    fn args(line: &str) -> Vec<String> {
        line.split_whitespace().map(String::from).collect()
    }

    #[test]
    fn the_command_line_picks_the_frontend_and_the_session() {
        assert_eq!(cli(&args("")).unwrap(), Cli::Tui(Resume::New));
        assert_eq!(cli(&args("--continue")).unwrap(), Cli::Tui(Resume::Latest));
        assert_eq!(
            cli(&args("--resume 0193")).unwrap(),
            Cli::Tui(Resume::Id("0193".into()))
        );
        assert_eq!(cli(&args("acp")).unwrap(), Cli::Acp);
        assert!(cli(&args("--resume")).is_err());
        let err = cli(&args("--continue extra")).unwrap_err();
        assert!(err.to_string().contains("--continue extra"), "{err}");
    }

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
        assert!(sessions_dir(|_| None, None).is_err());
    }

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
