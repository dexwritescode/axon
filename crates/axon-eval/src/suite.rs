//! Runs every task `runs` times, one run at a time, and sums them up.

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::Path;

use anyhow::Result;
use axon_protocol::SessionFactory;
use serde::Serialize;

use crate::run::{self, Limits, Outcome, Run};
use crate::task::Task;

/// A line of `results.jsonl`: how the run went, or why it couldn't finish.
#[derive(Serialize)]
struct Line<'a> {
    model: &'a str,
    task: &'a str,
    run: usize,
    #[serde(flatten)]
    outcome: Option<&'a Outcome>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
}

/// How one task went over its runs; None when it was broken.
pub struct Score {
    pub task: String,
    pub passes: Option<usize>,
    pub runs: usize,
}

/// Runs each task in `out/<task>-<n>`, appending a line per run to
/// `out/results.jsonl` as it finishes. A run that fails to finish counts as
/// failed and the suite goes on. Progress goes to stderr.
pub async fn suite(
    factory: &dyn SessionFactory,
    tasks: &[Task],
    runs: usize,
    model: &str,
    out: &Path,
    limits: Limits,
) -> Result<Vec<Score>> {
    let mut results = OpenOptions::new()
        .create(true)
        .append(true)
        .open(out.join("results.jsonl"))?;
    let mut scores = Vec::new();
    for task in tasks {
        let mut passes = Some(0);
        for n in 1..=runs {
            let name = &task.name;
            eprintln!("{name} {n}/{runs}");
            let dir = out.join(format!("{name}-{n}"));
            let line = |outcome, error| Line {
                model,
                task: name,
                run: n,
                outcome,
                error,
            };
            let line = match run::run(factory, task, &dir, limits).await {
                Ok(Run::Broken) => {
                    eprintln!("{name}: the check passes before the model runs; skipped");
                    fs::remove_dir_all(&dir)?;
                    passes = None;
                    break;
                }
                Ok(Run::Done(outcome)) => {
                    let pass = if outcome.pass { "pass" } else { "fail" };
                    eprintln!("{name} {n}/{runs}: {pass} in {:.0}s", outcome.secs);
                    if outcome.pass {
                        passes = passes.map(|p| p + 1);
                    }
                    serde_json::to_string(&line(Some(&outcome), None))?
                }
                Err(e) => {
                    let error = format!("{e:#}");
                    eprintln!("{name} {n}/{runs}: could not finish: {error}");
                    serde_json::to_string(&line(None, Some(error)))?
                }
            };
            writeln!(results, "{line}")?;
        }
        scores.push(Score {
            task: task.name.clone(),
            passes,
            runs,
        });
    }
    Ok(scores)
}

/// A table of pass counts per task, and the total.
pub fn summary(scores: &[Score]) -> String {
    let width = scores
        .iter()
        .map(|s| s.task.len())
        .max()
        .unwrap_or(0)
        .max(5);
    let mut text = String::new();
    let (mut passes, mut runs) = (0, 0);
    for score in scores {
        let result = match score.passes {
            Some(p) => {
                passes += p;
                runs += score.runs;
                format!("{p}/{}", score.runs)
            }
            None => "broken".into(),
        };
        text.push_str(&format!("{:width$}  {result}\n", score.task));
    }
    let percent = (passes * 100).checked_div(runs).unwrap_or(0);
    text.push_str(&format!(
        "{:width$}  {passes}/{runs} ({percent}%)\n",
        "total"
    ));
    text
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::run::LIMITS;
    use crate::run::tests::{Stub, task};

    fn lines(out: &Path) -> Vec<serde_json::Value> {
        let results = fs::read_to_string(out.join("results.jsonl")).unwrap();
        results
            .lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect()
    }

    fn out(root: &Path) -> std::path::PathBuf {
        let out = root.join("out");
        fs::create_dir(&out).unwrap();
        out
    }

    #[tokio::test]
    async fn each_run_gets_a_folder_and_a_result_line() {
        let root = tempfile::tempdir().unwrap();
        let out = out(root.path());
        let tasks = [task(root.path(), "a")];

        let scores = suite(&Stub::Fix, &tasks, 2, "m", &out, LIMITS)
            .await
            .unwrap();

        assert_eq!(scores[0].passes, Some(2));
        assert!(out.join("a-1/diff.patch").exists() && out.join("a-2/diff.patch").exists());
        let lines = lines(&out);
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[1]["model"], "m");
        assert_eq!(lines[1]["task"], "a");
        assert_eq!(lines[1]["run"], 2);
        assert_eq!(lines[1]["pass"], true);
        assert_eq!(lines[1]["stop"], "end_turn");
        assert!(lines[1].get("error").is_none());
    }

    #[tokio::test]
    async fn a_run_that_cannot_finish_fails_and_the_suite_goes_on() {
        let root = tempfile::tempdir().unwrap();
        let out = out(root.path());
        let mut broken = task(root.path(), "a");
        broken.spec.check = vec!["no-such-program-axon".into()];
        let tasks = [broken, task(root.path(), "b")];

        let scores = suite(&Stub::Fix, &tasks, 1, "m", &out, LIMITS)
            .await
            .unwrap();

        assert_eq!(scores[0].passes, Some(0));
        assert_eq!(scores[1].passes, Some(1));
        let lines = lines(&out);
        let error = lines[0]["error"].as_str().unwrap();
        assert!(error.contains("could not run the check"), "{error}");
        assert!(lines[0].get("pass").is_none());
        assert_eq!(lines[1]["pass"], true);
    }

    #[tokio::test]
    async fn a_broken_task_is_skipped_and_leaves_nothing() {
        let root = tempfile::tempdir().unwrap();
        let out = out(root.path());
        let tasks = [task(root.path(), "a")];
        fs::write(tasks[0].repo.join("notes.txt"), "fixed\n").unwrap();

        let scores = suite(&Stub::Fix, &tasks, 3, "m", &out, LIMITS)
            .await
            .unwrap();

        assert_eq!(scores[0].passes, None);
        assert!(!out.join("a-1").exists());
        assert_eq!(fs::read_to_string(out.join("results.jsonl")).unwrap(), "");
    }

    #[test]
    fn the_summary_counts_passes_and_leaves_broken_tasks_out_of_the_total() {
        let score = |task: &str, passes, runs| Score {
            task: task.into(),
            passes,
            runs,
        };
        let scores = [
            score("off-by-one", Some(2), 3),
            score("roman", Some(1), 3),
            score("x", None, 3),
        ];

        assert_eq!(
            summary(&scores),
            "off-by-one  2/3\nroman       1/3\nx           broken\ntotal       3/6 (50%)\n"
        );
    }
}
