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
| Ctrl+Enter, Ctrl+S | during a turn, steer it |
| Shift+Enter, Ctrl+J | new line |
| ←/→, ↑/↓ | move the cursor |
| ↑ on the first row, ↓ on the last | previous / next sent prompt |
| Alt+←/→, Ctrl+←/→, Alt+B/F | move by word |
| Home/End, Ctrl+A/E | line start/end |
| PageUp/PageDown, mouse wheel | scroll the history |
| Ctrl+End | jump the history to the bottom |
| Y / N | approve or reject a change |
| Esc, Ctrl+C | cancel the turn |
| Ctrl+C when idle, Ctrl+Q | quit |

Steering sends a prompt into the running turn: the model gets it before its next request, after the tool calls in flight. If the turn makes no further request (it ends, is cancelled or a change is rejected), the prompt starts the next turn. It shows as `Steer` below the turn until the model gets it.

Ctrl+Enter needs a terminal with the kitty keyboard protocol, for example kitty, WezTerm, Ghostty, Alacritty or foot. Other terminals, such as macOS Terminal and the Windows console, and tmux by default, send it as Enter (it queues) or as Ctrl+J (a new line). Ctrl+S steers in every terminal.

To use another server or model, set `OPENAI_BASE_URL`, `OPENAI_API_KEY` and `AXON_MODEL`, and name the server in `AXON_BACKEND`: `lmstudio` (the default), `ollama`, `llamacpp`, `vllm`, or `openai` for any other OpenAI-compatible server. Axon adapts its requests to the backend's quirks.

To let the machine cool between a turn's model requests, set `AXON_REQUEST_PAUSE_SECS` (default 0).

Each model reply may take at most `AXON_MAX_TOKENS` tokens, reasoning included (default 16384); a reply cut off there ends the turn.

A paste longer than `AXON_PASTE_TOKEN_CHARS` chars (default 1000) shows in the input and the history as one `[pasted N chars]` token; the model gets it in full.

At session start Axon asks the backend for the model's loaded context size (all but `openai` report it) and shows it after each turn, e.g. `✓ Finished at 14:32 · 2m 5s · 262k context`. Ollama and LM Studio report it only while the model is loaded.

## Test against real servers (macOS)

`scripts/backend-test.sh NAME` installs a backend with Homebrew or `lms`, starts it, loads a small Qwen3 0.6B model, runs the ignored live test (context discovery and a reply), then stops what it started. NAME is `ollama`, `llamacpp` or `lmstudio`. Downloads stay cached; `scripts/backends/NAME.sh purge` removes them. These scripts are macOS-only dev tooling.
