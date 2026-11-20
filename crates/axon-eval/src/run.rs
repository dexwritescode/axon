//! One run of one task: a fresh copy of its repo, one prompt, every tool call
//! approved, then the check.

use std::fs;
use std::io::{self, Read};
use std::path::Path;
use std::process::Stdio;
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use axon_protocol::{
    Command, Event, Exit, Resume, SessionFactory, SessionHandle, StopReason, ToolResult,
};
use process_wrap::tokio::*;
use serde::Serialize;
use tokio::sync::oneshot;

use crate::git::{git, git_ok};
use crate::task::Task;

/// How long a check and a model's turn may take.
#[derive(Debug, Clone, Copy)]
pub struct Limits {
    pub check: Duration,
    /// Past this the turn is cancelled.
    pub turn: Duration,
}

pub const LIMITS: Limits = Limits {
    // Room for a cold build of a workspace like Axon's.
    check: Duration::from_secs(600),
    turn: Duration::from_secs(30 * 60),
};

/// How long to keep reading a check's output after it is gone, in case
/// something outside its process group still holds the pipe.
const DRAIN: Duration = Duration::from_secs(1);

/// Build output deleted after a run; the source stays to read.
const BUILD_DIRS: [&str; 4] = ["target", "__pycache__", ".pytest_cache", ".venv"];

/// How a run went; a line of `results.jsonl`.
#[derive(Debug, Serialize)]
pub struct Outcome {
    /// The saved session. To open it: `cd <run>/work` and
    /// `AXON_SESSIONS_DIR=<out>/sessions axon --resume <id>`.
    pub session: String,
    /// The model changed something and the check passed after its turn,
    /// with the task's tests as they were.
    pub pass: bool,
    /// `cancelled` when the turn ran out of time; None when the session
    /// ended without ending the turn.
    pub stop: Option<StopReason>,
    /// The model changed the task's tests; they are put back before the check.
    pub tests_changed: bool,
    pub tool_calls: usize,
    /// Tool calls that returned `Failed`.
    pub tool_failures: usize,
    /// Commands that ended other than with exit code 0.
    pub command_failures: usize,
    /// The session's `Error` events.
    pub errors: Vec<String>,
    /// How long the model's turn took.
    pub secs: f64,
}

#[derive(Debug)]
pub enum Run {
    /// The check passed before the model started, so it proves nothing.
    Broken,
    Done(Outcome),
}

/// Runs `task` on a copy of its repo in `dir/work`, and leaves `check.txt`
/// (the check's output) and `diff.patch` (the model's changes) in `dir`.
pub async fn run(
    factory: &dyn SessionFactory,
    task: &Task,
    dir: &Path,
    limits: Limits,
) -> Result<Run> {
    let work = dir.join("work");
    let check = &task.spec.check;
    match &task.commit {
        Some(commit) => checkout(&task.repo, commit, dir).await?,
        None => copy_dir(&task.repo, &work)?,
    }
    clean(&work)?;
    git(&work, &["init", "-q"]).await?;
    git(&work, &["add", "-A"]).await?;
    git(&work, &["commit", "-q", "-m", "start"]).await?;
    let start = git(&work, &["rev-parse", "HEAD"]).await?;
    let start = start.trim();
    // A rev's check passes from the start: it asks that nothing broke.
    if task.commit.is_none() && run_check(check, &work, limits.check).await?.0 {
        clean(&work)?;
        return Ok(Run::Broken);
    }
    clean(&work)?;

    let session = factory.new_session(work.clone(), Resume::New, vec![])?;
    let id = session.id.clone();
    let began = Instant::now();
    let turn = drive(session, &task.spec.prompt, limits.turn).await;
    let secs = began.elapsed().as_secs_f64();

    clean(&work)?;
    git(&work, &["add", "-A"]).await?;
    let diff = git(&work, &["diff", "--cached", start]).await?;
    fs::write(dir.join("diff.patch"), &diff)?;
    let tests_changed = !task.spec.tests.is_empty()
        && !git_ok(
            &work,
            &with_paths(&["diff", "--cached", "--quiet", start], task),
        )
        .await?;
    if tests_changed {
        let source = format!("--source={start}");
        let restore = ["restore", &source, "--staged", "--worktree"];
        git(&work, &with_paths(&restore, task)).await?;
    }
    let changed = !git_ok(&work, &["diff", "--cached", "--quiet", start]).await?;

    let (passed, output) = run_check(check, &work, limits.check).await?;
    fs::write(dir.join("check.txt"), output)?;
    clean(&work)?;

    Ok(Run::Done(Outcome {
        session: id,
        pass: passed && changed,
        stop: turn.stop,
        tests_changed,
        tool_calls: turn.tool_calls,
        tool_failures: turn.tool_failures,
        command_failures: turn.command_failures,
        errors: turn.errors,
        secs,
    }))
}

/// `args`, then `--` and the task's test paths.
fn with_paths<'a>(args: &[&'a str], task: &'a Task) -> Vec<&'a str> {
    let tests = task.spec.tests.iter().map(String::as_str);
    args.iter().copied().chain(["--"]).chain(tests).collect()
}

#[derive(Debug, Default)]
struct Turn {
    stop: Option<StopReason>,
    tool_calls: usize,
    tool_failures: usize,
    command_failures: usize,
    errors: Vec<String>,
}

/// Sends `prompt`, approves every change, and counts what happens until the
/// turn ends; cancels it after `limit`. Then closes the session and waits for
/// it to finish saving. A session that goes away closes its events, which
/// ends the turn without a stop reason.
async fn drive(mut session: SessionHandle, prompt: &str, limit: Duration) -> Turn {
    let mut turn = Turn::default();
    let text = prompt.to_string();
    let _ = session.commands.send(Command::Prompt { text }).await;
    let deadline = tokio::time::sleep(limit);
    tokio::pin!(deadline);
    let mut cancelled = false;
    loop {
        let event = tokio::select! {
            event = session.events.recv() => event,
            () = &mut deadline, if !cancelled => {
                cancelled = true;
                let _ = session.commands.send(Command::Cancel).await;
                continue;
            }
        };
        let Some(event) = event else { break };
        match event {
            Event::ApprovalRequested { id, .. } => {
                let approve = Command::Approval { id, approve: true };
                let _ = session.commands.send(approve).await;
            }
            Event::ToolCallStarted { .. } => turn.tool_calls += 1,
            Event::ToolCallFinished { result, .. } => match result {
                ToolResult::Failed(_) => turn.tool_failures += 1,
                ToolResult::Command { exit, .. } if exit != Exit::Code(0) => {
                    turn.command_failures += 1
                }
                _ => {}
            },
            Event::Error { message } => turn.errors.push(message),
            Event::TurnEnded { stop } => {
                turn.stop = Some(stop);
                break;
            }
            _ => {}
        }
    }
    drop(session.commands);
    while session.events.recv().await.is_some() {}
    turn
}

/// Whether `check` passed in `work` within `limit`, and what it printed.
/// Whatever it started is killed when it ends or times out.
async fn run_check(check: &[String], work: &Path, limit: Duration) -> Result<(bool, String)> {
    let (mut reader, writer) = io::pipe()?;
    let stderr = writer.try_clone()?;
    let mut command = CommandWrap::with_new(&check[0], |c| {
        c.args(&check[1..])
            .current_dir(work)
            .stdin(Stdio::null())
            .stdout(writer)
            .stderr(stderr);
    });
    #[cfg(unix)]
    command.wrap(ProcessGroup::leader());
    #[cfg(windows)]
    command.wrap(JobObject);
    command.wrap(KillOnDrop);
    let mut child = command
        .spawn()
        .with_context(|| format!("could not run the check {check:?}"))?;
    drop(command);

    // A plain thread: something that escaped the group can hold the pipe
    // open forever.
    let (done, read) = oneshot::channel();
    thread::spawn(move || {
        let mut output = Vec::new();
        let _ = reader.read_to_end(&mut output);
        let _ = done.send(output);
    });
    let status = tokio::select! {
        status = child.wait() => Some(status?),
        () = tokio::time::sleep(limit) => None,
    };
    let _ = child.start_kill();
    let _ = child.wait().await;
    let output = match tokio::time::timeout(DRAIN, read).await {
        Ok(Ok(output)) => output,
        _ => Vec::new(),
    };
    let mut text = String::from_utf8_lossy(&output).into_owned();
    if status.is_none() {
        text.push_str(&format!("\n(timed out after {}s)\n", limit.as_secs()));
    }
    Ok((status.is_some_and(|s| s.success()), text))
}

/// Puts the files of `repo` at `commit` in `dir/work`, without its history.
/// The clone goes to a relative path: git may not take Windows' `\\?\` ones.
async fn checkout(repo: &Path, commit: &str, dir: &Path) -> Result<()> {
    fs::create_dir_all(dir)?;
    let repo = repo.to_string_lossy();
    let clone = ["clone", "-q", "--shared", "--no-checkout", &repo, "work"];
    git(dir, &clone).await?;
    let work = dir.join("work");
    git(&work, &["checkout", "-q", "--detach", commit]).await?;
    fs::remove_dir_all(work.join(".git"))?;
    Ok(())
}

fn copy_dir(from: &Path, to: &Path) -> Result<()> {
    fs::create_dir_all(to).with_context(|| format!("{}", to.display()))?;
    for entry in fs::read_dir(from).with_context(|| format!("{}", from.display()))? {
        let entry = entry?;
        let target = to.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_dir(&entry.path(), &target)?;
        } else {
            fs::copy(entry.path(), &target)
                .with_context(|| format!("{}", entry.path().display()))?;
        }
    }
    Ok(())
}

/// Deletes the build output under `dir`, keeping `.git`.
fn clean(dir: &Path) -> Result<()> {
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        if !entry.file_type()?.is_dir() || entry.file_name() == ".git" {
            continue;
        }
        if BUILD_DIRS.iter().any(|name| entry.file_name() == *name) {
            fs::remove_dir_all(entry.path())?;
        } else {
            clean(&entry.path())?;
        }
    }
    Ok(())
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::task::Spec;
    use axon_protocol::{Change, McpServer, SessionError};
    use std::path::PathBuf;
    use tokio::sync::mpsc;

    /// What the stub session does with the prompt. Each leaves a `target/`.
    #[derive(Debug, Clone, Copy)]
    pub(crate) enum Stub {
        /// Asks to write `fixed` into `notes.txt` and ends the turn.
        Fix,
        /// Fixes, then commits the fix itself.
        FixAndCommit,
        /// Reports a failed tool, a failed command and an error.
        Fail,
        /// Writes `fixed` into the test file instead.
        ChangeTests,
        /// Never ends the turn until it is cancelled.
        Hang,
        /// Goes away without ending the turn.
        Die,
    }

    impl SessionFactory for Stub {
        fn new_session(
            &self,
            cwd: PathBuf,
            _: Resume,
            _: Vec<McpServer>,
        ) -> Result<SessionHandle, SessionError> {
            let (commands, mut command_rx) = mpsc::channel(8);
            let (events, event_rx) = mpsc::channel(8);
            let stub = *self;
            tokio::spawn(async move {
                let Some(Command::Prompt { .. }) = command_rx.recv().await else {
                    return;
                };
                fs::create_dir_all(cwd.join("target/debug")).unwrap();
                let send = |event| {
                    let events = events.clone();
                    async move { events.send(event).await.unwrap() }
                };
                let stop = match stub {
                    Stub::Fix | Stub::FixAndCommit => {
                        let name = "write_file".into();
                        let args = r#"{"path":"notes.txt"}"#.into();
                        send(Event::ToolCallStarted { id: 1, name, args }).await;
                        let change = Change::WriteFile {
                            path: "notes.txt".into(),
                            old_text: None,
                            new_text: "fixed\n".into(),
                        };
                        send(Event::ApprovalRequested { id: 1, change }).await;
                        let Some(Command::Approval { approve: true, .. }) = command_rx.recv().await
                        else {
                            return;
                        };
                        fs::write(cwd.join("notes.txt"), "fixed\n").unwrap();
                        if let Stub::FixAndCommit = stub {
                            git(&cwd, &["commit", "-qam", "fix"]).await.unwrap();
                        }
                        let result = ToolResult::Output("ok".into());
                        send(Event::ToolCallFinished { id: 1, result }).await;
                        StopReason::EndTurn
                    }
                    Stub::Fail => {
                        let result = ToolResult::Failed("no such file".into());
                        send(Event::ToolCallFinished { id: 1, result }).await;
                        let (head, tail) = (String::new(), String::new());
                        let exit = Exit::Code(1);
                        let result = ToolResult::Command {
                            head,
                            cut: 0,
                            tail,
                            exit,
                        };
                        send(Event::ToolCallFinished { id: 2, result }).await;
                        send(Event::Error {
                            message: "oops".into(),
                        })
                        .await;
                        StopReason::EndTurn
                    }
                    Stub::ChangeTests => {
                        fs::write(cwd.join("tests/notes.txt"), "fixed\n").unwrap();
                        StopReason::EndTurn
                    }
                    Stub::Hang => {
                        let Some(Command::Cancel) = command_rx.recv().await else {
                            return;
                        };
                        StopReason::Cancelled
                    }
                    Stub::Die => return,
                };
                send(Event::TurnEnded { stop }).await;
            });
            let id = "s1".into();
            Ok(SessionHandle {
                id,
                commands,
                events: event_rx,
            })
        }
    }

    /// A task whose check (`git grep fixed`) fails until a tracked file says
    /// `fixed`; its tests are `tests/`.
    pub(crate) fn task(root: &Path, name: &str) -> Task {
        let repo = root.join(name);
        fs::create_dir_all(repo.join("src/__pycache__")).unwrap();
        fs::create_dir_all(repo.join("tests")).unwrap();
        fs::write(repo.join("notes.txt"), "broken\n").unwrap();
        fs::write(repo.join("tests/notes.txt"), "broken\n").unwrap();
        fs::write(repo.join("src/__pycache__/x.pyc"), "").unwrap();
        let spec = Spec {
            prompt: "fix".into(),
            rev: None,
            check: ["git", "grep", "-q", "fixed"].map(String::from).to_vec(),
            tests: vec!["tests".into()],
        };
        Task {
            name: name.into(),
            repo,
            commit: None,
            spec,
        }
    }

    const QUICK: Limits = Limits {
        check: Duration::from_secs(60),
        turn: Duration::from_millis(100),
    };

    /// Runs `stub` on a new task in `root/run`.
    async fn go(stub: Stub, root: &Path) -> Result<Run> {
        run(&stub, &task(root, "repo"), &root.join("run"), QUICK).await
    }

    fn done(run: Run) -> Outcome {
        match run {
            Run::Done(outcome) => outcome,
            Run::Broken => panic!("broken"),
        }
    }

    fn diff(root: &Path) -> String {
        fs::read_to_string(root.join("run/diff.patch")).unwrap()
    }

    #[tokio::test]
    async fn a_fixed_task_passes_and_leaves_its_diff_and_no_build_output() {
        let root = tempfile::tempdir().unwrap();

        let outcome = done(go(Stub::Fix, root.path()).await.unwrap());

        assert!(outcome.pass);
        assert!(!outcome.tests_changed);
        assert_eq!(outcome.session, "s1");
        assert_eq!(outcome.stop, Some(StopReason::EndTurn));
        assert_eq!(outcome.tool_calls, 1);
        let diff = diff(root.path());
        assert!(
            diff.contains("-broken") && diff.contains("+fixed"),
            "{diff}"
        );
        assert!(!diff.contains("target") && !diff.contains("pyc"), "{diff}");
        let dir = root.path().join("run");
        assert!(dir.join("check.txt").exists());
        assert!(!dir.join("work/target").exists());
        assert!(!dir.join("work/src/__pycache__").exists());
        assert!(dir.join("work/.git").exists());
    }

    #[tokio::test]
    async fn the_diff_is_against_the_start_even_when_the_model_commits() {
        let root = tempfile::tempdir().unwrap();

        let outcome = done(go(Stub::FixAndCommit, root.path()).await.unwrap());

        assert!(outcome.pass);
        assert!(diff(root.path()).contains("+fixed"));
    }

    #[tokio::test]
    async fn an_unfixed_task_fails_and_counts_what_went_wrong() {
        let root = tempfile::tempdir().unwrap();

        let outcome = done(go(Stub::Fail, root.path()).await.unwrap());

        assert!(!outcome.pass);
        assert_eq!(outcome.tool_failures, 1);
        assert_eq!(outcome.command_failures, 1);
        assert_eq!(outcome.errors, ["oops"]);
        assert_eq!(diff(root.path()), "");
    }

    #[tokio::test]
    async fn changed_tests_are_put_back_before_the_check() {
        let root = tempfile::tempdir().unwrap();

        let outcome = done(go(Stub::ChangeTests, root.path()).await.unwrap());

        assert!(outcome.tests_changed);
        assert!(!outcome.pass);
        assert!(diff(root.path()).contains("tests/notes.txt"));
        let tests = root.path().join("run/work/tests/notes.txt");
        assert_eq!(fs::read_to_string(tests).unwrap(), "broken\n");
    }

    #[tokio::test]
    async fn a_turn_past_its_time_is_cancelled() {
        let root = tempfile::tempdir().unwrap();

        let outcome = done(go(Stub::Hang, root.path()).await.unwrap());

        assert_eq!(outcome.stop, Some(StopReason::Cancelled));
        assert!(!outcome.pass);
    }

    #[tokio::test]
    async fn a_session_that_goes_away_has_no_stop_reason() {
        let root = tempfile::tempdir().unwrap();

        let outcome = done(go(Stub::Die, root.path()).await.unwrap());

        assert_eq!(outcome.stop, None);
        assert!(!outcome.pass);
    }

    #[tokio::test]
    async fn a_task_that_passes_before_the_model_runs_is_broken() {
        let root = tempfile::tempdir().unwrap();
        let task = task(root.path(), "repo");
        fs::write(task.repo.join("notes.txt"), "fixed\n").unwrap();

        let result = run(&Stub::Fix, &task, &root.path().join("run"), QUICK).await;

        assert!(matches!(result.unwrap(), Run::Broken));
    }

    /// A rev task starting from the first commit of a repo in `root/src`,
    /// where `notes.txt` says `broken`; a later commit adds `later.txt`, and
    /// there is uncommitted work on top. The check always passes.
    async fn rev_task(root: &Path) -> Task {
        let mut task = task(root, "src");
        task.spec.check = vec!["git".into(), "--version".into()];
        let repo = &task.repo;
        git(repo, &["init", "-q"]).await.unwrap();
        git(repo, &["add", "-A"]).await.unwrap();
        git(repo, &["commit", "-q", "-m", "one"]).await.unwrap();
        let first = git(repo, &["rev-parse", "HEAD"]).await.unwrap();
        fs::write(repo.join("later.txt"), "").unwrap();
        git(repo, &["add", "-A"]).await.unwrap();
        git(repo, &["commit", "-q", "-m", "two"]).await.unwrap();
        fs::write(repo.join("uncommitted.txt"), "").unwrap();
        task.commit = Some(first.trim().into());
        task
    }

    #[tokio::test]
    async fn a_rev_task_starts_from_that_commit_without_its_history() {
        let root = tempfile::tempdir().unwrap();
        let task = rev_task(root.path()).await;
        let dir = root.path().join("run");

        let outcome = done(run(&Stub::Fix, &task, &dir, QUICK).await.unwrap());

        assert!(outcome.pass);
        assert!(!dir.join("work/uncommitted.txt").exists());
        assert!(!dir.join("work/later.txt").exists());
        let log = git(&dir.join("work"), &["log", "--format=%s"])
            .await
            .unwrap();
        assert_eq!(log, "start\n");
        assert!(diff(root.path()).contains("+fixed"));
    }

    #[tokio::test]
    async fn a_run_that_changes_nothing_fails_though_the_check_passes() {
        let root = tempfile::tempdir().unwrap();
        let task = rev_task(root.path()).await;

        let outcome = run(&Stub::Die, &task, &root.path().join("run"), QUICK).await;

        assert!(!done(outcome.unwrap()).pass);
    }

    #[tokio::test]
    async fn a_rev_run_that_only_changes_the_tests_fails() {
        let root = tempfile::tempdir().unwrap();
        let task = rev_task(root.path()).await;

        let outcome = run(&Stub::ChangeTests, &task, &root.path().join("run"), QUICK).await;

        let outcome = done(outcome.unwrap());
        assert!(outcome.tests_changed);
        assert!(!outcome.pass);
    }

    #[tokio::test]
    async fn a_check_that_cannot_start_is_an_error() {
        let root = tempfile::tempdir().unwrap();
        let mut task = task(root.path(), "repo");
        task.spec.check = vec!["no-such-program-axon".into()];

        let result = run(&Stub::Fix, &task, &root.path().join("run"), QUICK).await;

        let err = result.unwrap_err().to_string();
        assert!(err.contains("could not run the check"), "{err}");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn a_check_past_its_time_fails_and_its_children_are_killed() {
        let root = tempfile::tempdir().unwrap();
        let script = "sleep 30 & echo $! > child.pid; echo started; wait";
        let check = ["sh", "-c", script].map(String::from);

        let began = Instant::now();
        let (passed, output) = run_check(&check, root.path(), Duration::from_millis(300))
            .await
            .unwrap();

        assert!(!passed);
        assert!(began.elapsed() < Duration::from_secs(5));
        assert!(
            output.starts_with("started\n") && output.contains("timed out"),
            "{output}"
        );
        let pid = fs::read_to_string(root.path().join("child.pid")).unwrap();
        let alive = std::process::Command::new("kill")
            .args(["-0", pid.trim()])
            .stderr(Stdio::null())
            .status()
            .unwrap();
        assert!(!alive.success(), "the check's child is still running");
    }
}
