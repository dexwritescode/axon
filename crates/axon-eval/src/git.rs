//! git with a fixed identity and without the user's git config, so their
//! hooks, excludes and line ending settings don't change a task or a run.

use std::path::Path;
use std::process::{Command, Output, Stdio};

use anyhow::{Context, Result, bail};

#[cfg(unix)]
const NO_CONFIG: &str = "/dev/null";
#[cfg(windows)]
const NO_CONFIG: &str = "NUL";

fn command(dir: &Path, args: &[&str]) -> Command {
    let mut command = Command::new("git");
    command
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", NO_CONFIG)
        .args([
            "-c",
            "user.name=axon-eval",
            "-c",
            "user.email=axon-eval@localhost",
        ])
        .args(args)
        .current_dir(dir)
        .stdin(Stdio::null());
    command
}

/// Runs git in `dir`; returns stdout, or fails when git does.
pub fn git_sync(dir: &Path, args: &[&str]) -> Result<String> {
    let output = command(dir, args).output().context("could not run git")?;
    stdout(args, output)
}

/// Runs git in `dir`; returns stdout, or fails when git does.
pub async fn git(dir: &Path, args: &[&str]) -> Result<String> {
    let output = tokio::process::Command::from(command(dir, args))
        .output()
        .await
        .context("could not run git")?;
    stdout(args, output)
}

/// Whether git exits 0, for the commands that answer with their exit code.
pub async fn git_ok(dir: &Path, args: &[&str]) -> Result<bool> {
    let output = tokio::process::Command::from(command(dir, args))
        .output()
        .await
        .context("could not run git")?;
    Ok(output.status.success())
}

fn stdout(args: &[&str], output: Output) -> Result<String> {
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!("git {}: {stderr}", args.join(" "));
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}
