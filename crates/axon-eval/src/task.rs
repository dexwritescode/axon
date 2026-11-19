//! Eval tasks: a folder each, holding `task.toml` and either the `repo/` the
//! model starts from or, with `rev`, a commit of the git repo the task is in.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use serde::Deserialize;

use crate::git::git_sync;

/// What `task.toml` holds.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Spec {
    /// Sent to the model as the only prompt.
    pub prompt: String,
    /// A commit of the git repo holding the task, to start from instead of `repo/`.
    pub rev: Option<String>,
    /// The program and its arguments; exit code 0 means the task is done.
    pub check: Vec<String>,
    /// Paths in the repo the check's tests live in; put back as they were
    /// before the check, so changing them doesn't pass it.
    #[serde(default)]
    pub tests: Vec<String>,
}

#[derive(Debug)]
pub struct Task {
    pub name: String,
    /// The starting files, copied for each run; with `rev`, the git repo
    /// they are checked out from.
    pub repo: PathBuf,
    /// The full id of the commit `rev` names, read once at load so a branch
    /// moving later doesn't change it.
    pub commit: Option<String>,
    pub spec: Spec,
}

/// Every task under `root`, by name.
pub fn load_all(root: &Path) -> Result<Vec<Task>> {
    let entries = fs::read_dir(root).with_context(|| format!("{}", root.display()))?;
    let mut tasks = Vec::new();
    for entry in entries {
        let dir = entry?.path();
        if dir.is_dir() {
            tasks.push(load(&dir)?);
        }
    }
    if tasks.is_empty() {
        bail!("{}: no tasks", root.display());
    }
    tasks.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(tasks)
}

fn load(dir: &Path) -> Result<Task> {
    let path = dir.join("task.toml");
    let text = fs::read_to_string(&path).with_context(|| format!("{}", path.display()))?;
    let spec: Spec = toml::from_str(&text).with_context(|| format!("{}", path.display()))?;
    if spec.check.is_empty() {
        bail!("{}: check is empty", path.display());
    }
    let (repo, commit) = match &spec.rev {
        Some(rev) => {
            let top = git_sync(dir, &["rev-parse", "--show-toplevel"])?;
            let commit = format!("{rev}^{{commit}}");
            let commit = git_sync(dir, &["rev-parse", "--verify", "--quiet", &commit])
                .with_context(|| format!("{}: no commit {rev}", path.display()))?;
            (PathBuf::from(top.trim()), Some(commit.trim().to_string()))
        }
        None => {
            let repo = dir.join("repo");
            if !repo.is_dir() {
                bail!("{}: no repo folder", dir.display());
            }
            (repo, None)
        }
    };
    let name = dir
        .file_name()
        .context("a task folder has no name")?
        .to_string_lossy()
        .into_owned();
    Ok(Task {
        name,
        repo,
        commit,
        spec,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn task(root: &Path, name: &str, toml: &str) {
        let dir = root.join(name);
        fs::create_dir_all(dir.join("repo")).unwrap();
        fs::write(dir.join("task.toml"), toml).unwrap();
    }

    #[test]
    fn tasks_load_in_name_order() {
        let root = tempfile::tempdir().unwrap();
        task(
            root.path(),
            "b",
            "prompt = \"fix b\"\ncheck = [\"true\"]\ntests = [\"tests\"]",
        );
        task(
            root.path(),
            "a",
            "prompt = \"fix a\"\ncheck = [\"cargo\", \"test\"]",
        );

        let tasks = load_all(root.path()).unwrap();

        assert_eq!(tasks[0].name, "a");
        assert_eq!(tasks[0].spec.prompt, "fix a");
        assert_eq!(tasks[0].spec.check, ["cargo", "test"]);
        assert!(tasks[0].spec.tests.is_empty());
        assert_eq!(tasks[1].name, "b");
        assert_eq!(tasks[1].spec.tests, ["tests"]);
    }

    #[test]
    fn a_bad_task_is_an_error_naming_it() {
        for (toml, expected) in [
            ("prompt = \"p\"\ncheck = []", "check is empty"),
            ("prompt = \"p\"", "task.toml"),
            ("prompt = \"p\"\ncheck = [\"x\"]\nextra = 1", "task.toml"),
        ] {
            let root = tempfile::tempdir().unwrap();
            task(root.path(), "t", toml);
            let err = format!("{:#}", load_all(root.path()).unwrap_err());
            assert!(err.contains(expected), "{err}");
        }
    }

    #[test]
    fn a_rev_task_starts_from_the_git_repo_it_is_in() {
        let root = tempfile::tempdir().unwrap();
        let top = root.path().canonicalize().unwrap();
        for args in [
            &["init", "-q"][..],
            &["commit", "-q", "--allow-empty", "-m", "one"],
            &["branch", "first"],
            &["commit", "-q", "--allow-empty", "-m", "two"],
        ] {
            git_sync(&top, args).unwrap();
        }
        let id = |rev| {
            git_sync(&top, &["rev-parse", rev])
                .unwrap()
                .trim()
                .to_string()
        };
        let tasks = top.join("tasks");
        let dir = tasks.join("t");
        fs::create_dir_all(&dir).unwrap();
        let toml = |rev| format!("prompt = \"p\"\nrev = \"{rev}\"\ncheck = [\"x\"]");

        fs::write(dir.join("task.toml"), toml("HEAD")).unwrap();
        let task = &load_all(&tasks).unwrap()[0];
        assert_eq!(task.repo.canonicalize().unwrap(), top);
        assert_eq!(task.spec.rev.as_deref(), Some("HEAD"));
        for rev in ["HEAD", "HEAD^", "first"] {
            fs::write(dir.join("task.toml"), toml(rev)).unwrap();
            let task = &load_all(&tasks).unwrap()[0];
            assert_eq!(task.commit, Some(id(rev)), "{rev}");
        }

        fs::write(dir.join("task.toml"), toml("nope")).unwrap();
        let err = format!("{:#}", load_all(&tasks).unwrap_err());
        assert!(err.contains("no commit nope"), "{err}");
    }

    #[test]
    fn a_task_without_a_repo_or_no_tasks_is_an_error() {
        let root = tempfile::tempdir().unwrap();
        assert!(
            load_all(root.path())
                .unwrap_err()
                .to_string()
                .contains("no tasks")
        );

        fs::create_dir(root.path().join("t")).unwrap();
        fs::write(
            root.path().join("t/task.toml"),
            "prompt = \"p\"\ncheck = [\"x\"]",
        )
        .unwrap();
        let err = load_all(root.path()).unwrap_err().to_string();
        assert!(err.contains("no repo folder"), "{err}");
    }
}
