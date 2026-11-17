//! `run_command`: run a command in the platform shell, once the user approves.

use std::collections::VecDeque;
use std::io::{self, Read};
use std::path::Path;
use std::process::Stdio;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axon_protocol::Exit;
use process_wrap::tokio::*;
use serde_json::Value;
use tokio_util::sync::CancellationToken;

use super::{optional_usize, required_str};

const DEFAULT_TIMEOUT_SECS: u64 = 120;
const MAX_TIMEOUT_SECS: u64 = 600;
const HEAD_BYTES: usize = 8 * 1024;
const TAIL_BYTES: usize = 24 * 1024;
/// How long to keep reading after the command is gone, in case something
/// outside its process group still holds the output pipe.
const DRAIN: Duration = Duration::from_secs(1);

#[cfg(unix)]
pub(super) const SHELL: &str = "sh -c";
#[cfg(windows)]
pub(super) const SHELL: &str = "cmd /C";

/// A command the model wants to run.
#[derive(Debug, PartialEq)]
pub(crate) struct Run {
    pub(crate) command: String,
    pub(crate) timeout_secs: u64,
}

pub(super) fn run_command(args: &Value) -> Result<Run, String> {
    let command = required_str(args, "command")?;
    if command.trim().is_empty() {
        return Err("command is empty".into());
    }
    let timeout_secs = optional_usize(args, "timeout")?
        .map_or(DEFAULT_TIMEOUT_SECS, |secs| secs as u64)
        .clamp(1, MAX_TIMEOUT_SECS);
    Ok(Run {
        command: command.into(),
        timeout_secs,
    })
}

impl Run {
    /// Run the command in `cwd` until it exits, times out or `cancel` fires;
    /// then kill whatever is left of it. Returns what it printed (capped) and
    /// how it ended, or why it couldn't start.
    pub(crate) async fn execute(
        &self,
        cwd: &Path,
        cancel: &CancellationToken,
    ) -> Result<(String, Exit), String> {
        let (reader, writer) = io::pipe().map_err(|e| e.to_string())?;
        let mut command = shell(&self.command);
        command.command_mut().current_dir(cwd).stdin(Stdio::null());
        command
            .command_mut()
            .stdout(writer.try_clone().map_err(|e| e.to_string())?)
            .stderr(writer);
        let mut child = command
            .spawn()
            .map_err(|e| format!("could not start {SHELL}: {e}"))?;
        drop(command);

        let capture = Arc::new(Mutex::new(Capture::default()));
        let reading = tokio::task::spawn_blocking({
            let capture = Arc::clone(&capture);
            move || read_into(reader, &capture)
        });

        let exit = tokio::select! {
            status = child.wait() => match status.map(|s| s.code()) {
                Ok(Some(code)) => Exit::Code(code),
                Ok(None) | Err(_) => Exit::Killed,
            },
            _ = tokio::time::sleep(Duration::from_secs(self.timeout_secs)) => {
                Exit::TimedOut { secs: self.timeout_secs }
            }
            _ = cancel.cancelled() => Exit::Cancelled,
        };
        let _ = child.start_kill();
        let _ = child.wait().await;
        let _ = tokio::time::timeout(DRAIN, reading).await;

        let output = capture.lock().unwrap().text();
        Ok((output, exit))
    }
}

#[cfg(unix)]
fn shell(command: &str) -> CommandWrap {
    let mut wrap = CommandWrap::with_new("sh", |c| {
        c.arg("-c").arg(command);
    });
    wrap.wrap(ProcessGroup::leader()).wrap(KillOnDrop);
    wrap
}

#[cfg(windows)]
fn shell(command: &str) -> CommandWrap {
    let mut wrap = CommandWrap::with_new("cmd", |c| {
        c.arg("/C").raw_arg(command);
    });
    wrap.wrap(JobObject).wrap(KillOnDrop);
    wrap
}

fn read_into(mut reader: io::PipeReader, capture: &Mutex<Capture>) {
    let mut buf = [0; 8192];
    while let Ok(n @ 1..) = reader.read(&mut buf) {
        capture.lock().unwrap().push(&buf[..n]);
    }
}

/// The first `HEAD_BYTES` and last `TAIL_BYTES` of a stream, and its length.
#[derive(Default)]
struct Capture {
    head: Vec<u8>,
    tail: VecDeque<u8>,
    total: usize,
}

impl Capture {
    fn push(&mut self, mut bytes: &[u8]) {
        self.total += bytes.len();
        let room = HEAD_BYTES - self.head.len();
        let (head, rest) = bytes.split_at(room.min(bytes.len()));
        self.head.extend_from_slice(head);
        bytes = rest;
        self.tail.extend(bytes);
        let over = self.tail.len().saturating_sub(TAIL_BYTES);
        self.tail.drain(..over);
    }

    fn text(&self) -> String {
        let head = String::from_utf8_lossy(&self.head);
        let tail: Vec<u8> = self.tail.iter().copied().collect();
        let tail = String::from_utf8_lossy(&tail);
        let cut = self.total - self.head.len() - self.tail.len();
        if cut == 0 {
            format!("{head}{tail}")
        } else {
            format!("{head}\n[… {cut} bytes cut …]\n{tail}")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use tempfile::TempDir;

    async fn run(command: &str, timeout_secs: u64) -> (String, Exit) {
        let dir = TempDir::new().unwrap();
        let run = Run {
            command: command.into(),
            timeout_secs,
        };
        run.execute(dir.path(), &CancellationToken::new())
            .await
            .unwrap()
    }

    #[test]
    fn args_need_a_command_and_clamp_the_timeout() {
        assert_eq!(run_command(&json!({})), Err("command is required".into()));
        assert_eq!(
            run_command(&json!({"command": " "})),
            Err("command is empty".into())
        );
        let timeout = |args| run_command(&args).unwrap().timeout_secs;
        assert_eq!(timeout(json!({"command": "ls"})), DEFAULT_TIMEOUT_SECS);
        assert_eq!(timeout(json!({"command": "ls", "timeout": 0})), 1);
        assert_eq!(
            timeout(json!({"command": "ls", "timeout": 9999})),
            MAX_TIMEOUT_SECS
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn output_and_exit_code_come_back() {
        assert_eq!(
            run("echo out; echo err >&2; exit 3", 10).await,
            ("out\nerr\n".into(), Exit::Code(3))
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn runs_in_the_working_directory() {
        let dir = TempDir::new().unwrap();
        std::fs::write(dir.path().join("here.txt"), "").unwrap();
        let run = Run {
            command: "ls".into(),
            timeout_secs: 10,
        };

        let (output, _) = run
            .execute(dir.path(), &CancellationToken::new())
            .await
            .unwrap();
        assert_eq!(output, "here.txt\n");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn a_timeout_kills_the_command_and_its_children() {
        let (output, exit) = run("echo started; sleep 30 & wait", 1).await;

        assert_eq!(
            (output.as_str(), exit),
            ("started\n", Exit::TimedOut { secs: 1 })
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn cancel_kills_the_command() {
        let dir = TempDir::new().unwrap();
        let cancel = CancellationToken::new();
        let run = Run {
            command: "sleep 30".into(),
            timeout_secs: 60,
        };

        let (result, ()) = tokio::join!(run.execute(dir.path(), &cancel), async {
            tokio::time::sleep(Duration::from_millis(100)).await;
            cancel.cancel();
        });
        assert_eq!(result, Ok((String::new(), Exit::Cancelled)));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn background_jobs_are_killed_when_the_command_exits() {
        let started = std::time::Instant::now();

        let (_, exit) = run("sleep 30 & echo done", 60).await;

        assert_eq!(exit, Exit::Code(0));
        assert!(started.elapsed() < Duration::from_secs(5));
    }

    #[test]
    fn long_output_keeps_its_head_and_tail() {
        let mut capture = Capture::default();
        capture.push(&vec![b'h'; HEAD_BYTES]);
        capture.push(&[b'x'; 100]);
        capture.push(&vec![b't'; TAIL_BYTES]);

        let text = capture.text();
        assert_eq!(
            text,
            format!(
                "{}\n[… 100 bytes cut …]\n{}",
                "h".repeat(HEAD_BYTES),
                "t".repeat(TAIL_BYTES)
            )
        );
    }

    #[test]
    fn invalid_utf8_is_replaced() {
        let mut capture = Capture::default();
        capture.push(b"ok \xff\n");

        assert_eq!(capture.text(), "ok \u{fffd}\n");
    }
}
