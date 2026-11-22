# Evals

`axon-eval` runs coding tasks through Axon on the configured model and checks the results.

```sh
cargo run -p axon-eval -- evals/tasks evals/runs/NAME [--runs N]
```

It runs each task in the folder `N` times (default once), one run at a time, and approves every tool call. It uses the same [configuration](configuration.md) as Axon. A pass-rate summary prints at the end.

## Task sets

| Folder | Tasks |
| --- | --- |
| `evals/tasks` | Small bug fixes in Rust and Python. The Python tasks need `uv` |
| `evals/axon` | Changes from Axon's own history, starting at the commit before each. The check is that the workspace's tests pass |

## Writing a task

A task is a folder with a `task.toml`:

```toml
prompt = "The tests fail (run `uv run --no-project python -m unittest`). Fix the bugs in the code, not the tests."
check = ["uv", "run", "--no-project", "python", "-m", "unittest", "--quiet"]
tests = ["test_totals.py"]
```

| Field | Meaning |
| --- | --- |
| `prompt` | What the model is asked to do |
| `check` | The command that passes once the task is done |
| `tests` | Test files to put back before the check, in case the model changed them |
| `rev` | A commit of this repo to start from |

Without `rev`, the model starts from the task's `repo/` folder.

## Scoring

- A run that changes nothing fails.
- For a `rev` task, any change that keeps the tests passing counts, so read its diff.

## Output

Each run leaves its `work/` folder, `diff.patch` and `check.txt` in `evals/runs/NAME/<task>-<n>`, and adds a line to `evals/runs/NAME/results.jsonl`.

The runs share one cargo build folder, `<out>/target`, which is deleted at the end or on Ctrl+C. `CARGO_TARGET_DIR` must be unset.
