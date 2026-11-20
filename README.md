# Axon

[![Build](https://github.com/dexwritescode/axon/actions/workflows/build.yml/badge.svg?branch=main)](https://github.com/dexwritescode/axon/actions/workflows/build.yml)
[![Test](https://github.com/dexwritescode/axon/actions/workflows/test.yml/badge.svg?branch=main)](https://github.com/dexwritescode/axon/actions/workflows/test.yml)

Axon is an AI coding agent.

## Run against a local model

Axon talks to any OpenAI-compatible server. By default it uses LM Studio's local server and Qwen3.6 35B A3B.

```sh
lms server start                  # OpenAI-compatible server on localhost:1234
lms load qwen/qwen3.6-35b-a3b     # pass --context-length 32768 to set the context size
lms ps                            # what's loaded
cargo run                         # the TUI
lms unload --all && lms server stop
```

TUI keys:

| Key | Action |
| --- | --- |
| Enter | send; during a turn, send when it ends |
| Ctrl+Enter, Ctrl+S | during a turn, stop it and send what waits now |
| Shift+Enter, Ctrl+J | new line |
| ←/→, ↑/↓ | move the cursor |
| ↑ on the first row, ↓ on the last | previous / next sent prompt |
| Alt+←/→, Ctrl+←/→, Alt+B/F | move by word |
| Home/End, Ctrl+A/E | line start/end |
| PageUp/PageDown, mouse wheel | scroll the history |
| Ctrl+End | jump the history to the bottom |
| Y / N | approve or reject a change |
| Esc, Ctrl+C | cancel the turn; what waits is sent |
| Ctrl+C when idle, Ctrl+Q | quit |

Prompts sent during a turn wait below it as `Next`. When the turn ends, however it ends, they go to the model together as one message. Ctrl+Enter adds the input to them and stops the turn at once, keeping the reply so far.

Ctrl+Enter needs a terminal with the kitty keyboard protocol, for example kitty, WezTerm, Ghostty, iTerm2, Alacritty or foot. On Windows it works in any console. Other terminals, such as macOS Terminal, and tmux by default, send it as Enter (it queues) or as Ctrl+J (a new line). Ctrl+S works in every terminal.

Each session is saved when a turn ends, in `~/.axon/sessions` (`%USERPROFILE%\.axon\sessions` on Windows) or in `AXON_SESSIONS_DIR`. Quitting prints the command to resume it. `axon --continue` resumes the session last used in the current folder and `axon --resume <id>` the one whose id starts with `<id>`; the history shows again and the model gets the conversation so far.

To use another server or model, set `OPENAI_BASE_URL`, `OPENAI_API_KEY` and `AXON_MODEL`, and name the server in `AXON_BACKEND`: `lmstudio` (the default), `ollama`, `llamacpp`, `vllm`, or `openai` for any other OpenAI-compatible server. Axon adapts its requests to the backend's quirks.

To let the machine cool between a turn's model requests, set `AXON_REQUEST_PAUSE_SECS` (default 0). Set `AXON_REQUEST_PAUSE_RATIO` too and the pause follows the request before it instead: that many times as long as it took, at most `AXON_REQUEST_PAUSE_SECS` (no cap when 0). With `0.5` and `30`, a 10 s request is followed by 5 s, a 90 s one by 30 s.

Each model reply may take at most `AXON_MAX_TOKENS` tokens, reasoning included (default 16384); a reply cut off there ends the turn. `AXON_REASONING_EFFORT` (`none`, `low`, `medium` or `high`) is sent with each request when set; `none` turns thinking off where the server supports it (LM Studio does for qwen); a server may reject a value it doesn't support.

`AXON_PROFILE` names a TOML file that changes the agent, for example:

```toml
system_prompt = """
You are BUDDY, a voice assistant. Answer in one or two short sentences.
"""
tools = ["read_file", "grep"]   # built-in tools to offer; [] offers none; leave it out for all
```

Both fields are optional: `system_prompt` replaces Axon's own, and `tools` limits which built-in tools the model gets: `read_file`, `list_dir`, `grep`, `find`, `write_file`, `edit_file`, `run_command`. A profile that can't be read or parsed, or names an unknown field or tool, stops Axon with an error.

A paste longer than `AXON_PASTE_TOKEN_CHARS` chars (default 1000) shows in the input and the history as one `[pasted N chars]` token; the model gets it in full.

At session start Axon asks the backend for the model's loaded context size (all but `openai` report it) and shows it after each turn, e.g. `✓ Finished at 14:32 · 2m 5s · 262k context`. Ollama and LM Studio report it only while the model is loaded.

## ACP (Zed and other clients)

`axon acp` speaks the [Agent Client Protocol](https://agentclientprotocol.com/) over stdio, with the same env vars and `AXON_PROFILE` as the TUI. In Zed's settings:

```json
"agent_servers": { "Axon": { "command": "axon", "args": ["acp"] } }
```

It supports `session/new`, `session/prompt` (text only), `session/cancel`, streamed replies, reasoning and tool calls, and asks for approval of writes (with a diff) and commands through `session/request_permission`. One prompt runs at a time per session; another sent meanwhile is an error. MCP servers sent with `session/new` are ignored for now.

## Test against real servers (macOS)

`scripts/backend-test.sh NAME` installs a backend with Homebrew or `lms`, starts it, loads a small Qwen3 0.6B model, runs the ignored live test (context discovery and a reply), then stops what it started. NAME is `ollama`, `llamacpp` or `lmstudio`. Downloads stay cached; `scripts/backends/NAME.sh purge` removes them. These scripts are macOS-only dev tooling.

## Evals

`cargo run -p axon-eval -- evals/tasks evals/runs/NAME [--runs N]` runs each task in `evals/tasks` through Axon on the configured model (the same env vars as above), one run at a time, approving every tool call. A task is a `task.toml` (the prompt, the `check` command that passes once the task is done, the `tests` paths) and the `repo/` the model starts from, or a `rev` of this repo to start from instead. `evals/tasks` holds small bug fixes in Rust and Python; `evals/axon` holds changes from Axon's own history, starting at the commit before each, whose check is that the workspace's tests still pass. A run that changes nothing fails. For a `rev` task any change that keeps the tests passing counts, so read its diff. The runs share one cargo build folder, `<out>/target`, deleted at the end or on Ctrl-C; `CARGO_TARGET_DIR` must be unset. Each run leaves its `work/` folder, `diff.patch` and `check.txt` in `evals/runs/NAME/<task>-<n>`, and a line in `results.jsonl`; a pass-rate summary prints at the end. Tests the model changes are put back before the check. The Python tasks need `uv`.
