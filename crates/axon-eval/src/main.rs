//! Runs eval tasks through Axon on the configured model and scores them.
//! The runs only talk to sessions through `axon-protocol`, like a frontend.

mod run;
mod suite;
mod task;

use std::env;
use std::fs;
use std::path::PathBuf;

use anyhow::{Context, Result, bail};
use axon_config::Config;
use axon_core::CoreFactory;

const USAGE: &str = "usage: axon-eval <tasks> <out> [--runs N]";

/// What the command line asks for.
#[derive(Debug, PartialEq)]
struct Cli {
    tasks: PathBuf,
    /// Created by the run; must not exist yet.
    out: PathBuf,
    runs: usize,
}

#[tokio::main]
async fn main() -> Result<()> {
    let args: Vec<String> = env::args().skip(1).collect();
    let cli = cli(&args)?;
    let tasks = task::load_all(&cli.tasks)?;
    let config = Config::from_env()?;
    if let Some(parent) = cli.out.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::create_dir(&cli.out).with_context(|| format!("{}", cli.out.display()))?;
    let out = cli.out.canonicalize()?;
    // Cargo finds this walking up from each run, so the runs share one build
    // folder instead of each building from nothing.
    fs::create_dir(out.join(".cargo"))?;
    fs::write(
        out.join(".cargo/config.toml"),
        "[build]\ntarget-dir = \"target\"\n",
    )?;
    let model = config.model.model.clone();
    let factory = CoreFactory::new(config.model, out.join("sessions"));
    let scores = suite::suite(&factory, &tasks, cli.runs, &model, &out, run::LIMITS).await;
    let target = out.join("target");
    if target.exists() {
        fs::remove_dir_all(target)?;
    }
    let scores = scores?;
    eprint!("\n{}", suite::summary(&scores));
    Ok(())
}

fn cli(args: &[String]) -> Result<Cli> {
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    let (tasks, out, runs) = match args[..] {
        [tasks, out] => (tasks, out, "1"),
        [tasks, out, "--runs", runs] => (tasks, out, runs),
        _ => bail!(USAGE),
    };
    let runs = match runs.parse() {
        Ok(0) | Err(_) => bail!("--runs: not a whole number above 0: {runs}"),
        Ok(runs) => runs,
    };
    Ok(Cli {
        tasks: tasks.into(),
        out: out.into(),
        runs,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(line: &str) -> Vec<String> {
        line.split_whitespace().map(String::from).collect()
    }

    #[test]
    fn the_command_line_names_the_tasks_the_output_and_the_runs() {
        let cli = |line| super::cli(&args(line));
        let expected = |runs| Cli {
            tasks: "t".into(),
            out: "o".into(),
            runs,
        };

        assert_eq!(cli("t o").unwrap(), expected(1));
        assert_eq!(cli("t o --runs 3").unwrap(), expected(3));
        assert_eq!(cli("t").unwrap_err().to_string(), USAGE);
        assert!(
            cli("t o --runs 0")
                .unwrap_err()
                .to_string()
                .contains("--runs")
        );
        assert!(cli("t o --runs x").is_err());
    }
}
