//! Runs eval tasks through Axon on the configured model and scores them.
//! The runs only talk to sessions through `axon-protocol`, like a frontend.

mod git;
mod run;
mod suite;
mod task;

use std::env;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow, bail};
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
    if env::var_os("CARGO_TARGET_DIR").is_some_and(|dir| !dir.is_empty()) {
        bail!("unset CARGO_TARGET_DIR: the runs share a build folder in <out>/target");
    }
    let tasks = task::load_all(&cli.tasks)?;
    let config = Config::from_env()?;
    if let Some(parent) = cli.out.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::create_dir(&cli.out).with_context(|| format!("{}", cli.out.display()))?;
    let out = cli.out.canonicalize()?;
    share_build(&out)?;
    let model = config.model.model.clone();
    let factory = CoreFactory::new(config.model, config.agent, out.join("sessions"));
    let scores = tokio::select! {
        scores = suite::suite(&factory, &tasks, cli.runs, &model, &out, run::LIMITS) => scores,
        _ = tokio::signal::ctrl_c() => Err(anyhow!("interrupted")),
    };
    let unshared = unshare_build(&out);
    let scores = scores?;
    unshared?;
    eprint!("\n{}", suite::summary(&scores));
    Ok(())
}

/// Points cargo, which finds `<out>/.cargo` walking up from each run, at one
/// build folder for all runs instead of each building from nothing.
fn share_build(out: &Path) -> Result<()> {
    fs::create_dir(out.join(".cargo"))?;
    fs::write(
        out.join(".cargo/config.toml"),
        "[build]\ntarget-dir = \"target\"\n",
    )?;
    Ok(())
}

/// Deletes the shared build folder and stops pointing cargo at it.
fn unshare_build(out: &Path) -> Result<()> {
    for dir in ["target", ".cargo"] {
        let dir = out.join(dir);
        if dir.exists() {
            fs::remove_dir_all(&dir).with_context(|| format!("{}", dir.display()))?;
        }
    }
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
    fn the_runs_share_a_build_folder_that_goes_at_the_end() {
        let out = tempfile::tempdir().unwrap();
        share_build(out.path()).unwrap();
        let config = fs::read_to_string(out.path().join(".cargo/config.toml")).unwrap();
        assert!(config.contains(r#"target-dir = "target""#), "{config}");
        fs::create_dir_all(out.path().join("target/debug")).unwrap();

        unshare_build(out.path()).unwrap();

        assert!(!out.path().join("target").exists());
        assert!(!out.path().join(".cargo").exists());
        unshare_build(out.path()).unwrap();
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
