use std::env;
use std::sync::Arc;

use anyhow::{Result, bail};
use axon_core::CoreFactory;
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
            let config = axon_config::Config::from_env()?;
            let settings = tui_settings(config.paste_token_chars);
            let factory = CoreFactory::new(config.model, config.agent, config.sessions);
            axon_tui::run(&factory, settings, resume).await?
        }
        Cli::Acp => {
            let config = axon_config::Config::from_env()?;
            let factory = CoreFactory::new(config.model, config.agent, config.sessions);
            axon_acp::serve(Arc::new(factory)).await?
        }
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

/// The TUI's defaults, with what the config sets.
fn tui_settings(paste_token_chars: Option<usize>) -> axon_tui::Settings {
    let mut settings = axon_tui::Settings::default();
    if let Some(chars) = paste_token_chars {
        settings.paste_token_chars = chars;
    }
    settings
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
    fn tui_settings_keep_the_default_unless_the_config_sets_it() {
        assert_eq!(tui_settings(None).paste_token_chars, 1000);
        assert_eq!(tui_settings(Some(50)).paste_token_chars, 50);
    }
}
