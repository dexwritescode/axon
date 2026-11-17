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
| Enter | send |
| Shift+Enter, Ctrl+J | new line |
| ←/→, ↑/↓ | move the cursor |
| Alt+←/→, Ctrl+←/→, Alt+B/F | move by word |
| Home/End, Ctrl+A/E | line start/end |
| PageUp/PageDown, mouse wheel | scroll the history |
| Ctrl+End | jump the history to the bottom |
| Y / N | approve or reject a change |
| Esc, Ctrl+C | cancel the turn |
| Ctrl+C when idle, Ctrl+Q | quit |

To use another server or model, set `OPENAI_BASE_URL`, `OPENAI_API_KEY` and `AXON_MODEL`.

To let the machine cool between a turn's model requests, set `AXON_REQUEST_PAUSE_SECS` (default 0).
