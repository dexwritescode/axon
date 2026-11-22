# Configuration

Axon is configured with environment variables. The terminal UI and `axon acp` read the same ones.

## Server and model

| Variable | Default | Meaning |
| --- | --- | --- |
| `AXON_BACKEND` | `lmstudio` | The server: `lmstudio`, `ollama`, `llamacpp`, `vllm`, or `openai` for any other OpenAI-compatible server |
| `OPENAI_BASE_URL` | `http://localhost:1234/v1` | The server's URL |
| `OPENAI_API_KEY` | `lm-studio` | The API key |
| `AXON_MODEL` | `qwen/qwen3.6-35b-a3b` | The model |

Axon adapts its requests to each backend's quirks, so set `AXON_BACKEND` to match the server.

To set the context size in LM Studio, load the model with `lms load qwen/qwen3.6-35b-a3b --context-length 32768`. `lms ps` shows what's loaded.

## Model replies

| Variable | Default | Meaning |
| --- | --- | --- |
| `AXON_MAX_TOKENS` | `16384` | Most tokens per reply, reasoning included. A reply cut off here ends the turn |
| `AXON_REASONING_EFFORT` | unset | `none`, `low`, `medium` or `high`, sent with each request |

`none` turns thinking off where the server supports it, as LM Studio does for Qwen. A server may reject a value it doesn't support.

## Pausing between requests

A turn can make many model requests in a row. To let the machine cool between them, set a pause.

| Variable | Default | Meaning |
| --- | --- | --- |
| `AXON_REQUEST_PAUSE_SECS` | `0` | Seconds to wait before each request after a turn's first |
| `AXON_REQUEST_PAUSE_RATIO` | unset | Wait this many times as long as the request took instead, capped at `AXON_REQUEST_PAUSE_SECS` (no cap when it is `0`) |

For example, with a ratio of `0.5` and `30` seconds, a 10 s request is followed by a 5 s pause and a 90 s request by a 30 s pause.

## Other settings

| Variable | Default | Meaning |
| --- | --- | --- |
| `AXON_PROFILE` | unset | A TOML file that changes the agent; see [Profiles](profiles.md) |
| `AXON_SESSIONS_DIR` | `~/.axon/sessions` | Where sessions are saved; see [Terminal UI](tui.md#sessions) |
| `AXON_PASTE_TOKEN_CHARS` | `1000` | In the terminal UI, pastes longer than this show as one token; see [Terminal UI](tui.md#pastes) |
