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

In the TUI, Enter sends, Shift+Enter or Ctrl+J adds a newline, and Ctrl+C or Ctrl+Q quits.

To use another server or model, set `OPENAI_BASE_URL`, `OPENAI_API_KEY` and `AXON_MODEL`.
