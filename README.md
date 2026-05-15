# Axon

![Build](https://github.com/dexwritescode/axon/actions/workflows/build.yml/badge.svg)
![Test](https://github.com/dexwritescode/axon/actions/workflows/test.yml/badge.svg)

Axon is a terminal AI coding agent. You describe a goal; it reads files, edits code, runs shell commands, and streams every step back to you in real time — then waits for your review before committing any change to disk.

It speaks OpenAI-compatible HTTP, so it works with any backend: [Neurons](https://github.com/dexwritescode/neurons), Ollama, LM Studio, vLLM, or a hosted endpoint. Nothing leaves your machine unless you point Axon at a remote host.

---

## Design

**Infinite canvas, not a pop-up.** Axon renders directly into the terminal's normal scrollback buffer. There is no alternate screen, no modal overlay. Every message, tool call, and diff accumulates as permanent history above a fixed input bar at the bottom — exactly like a chat interface, but in your terminal.

**You approve every edit.** When the model proposes a file change, Axon shows the full syntax-highlighted diff and waits for `y` or `n` before writing anything. Accepting commits a compact version of the diff into the scrollback so you have a permanent audit trail.

**Tool approval modes.** The `tool_approval` config key controls how non-edit tools (shell commands, file reads) are handled: `allow` runs them automatically, `ask` prompts before each one, `deny` blocks them entirely.

---

## Install

Download the latest binary for your platform from the [Releases](../../releases) page:

```bash
# macOS (Apple Silicon)
curl -Lo axon https://github.com/dexwritescode/axon/releases/latest/download/axon-latest-macos-arm64
chmod +x axon && mv axon /usr/local/bin/

# Linux (amd64)
curl -Lo axon https://github.com/dexwritescode/axon/releases/latest/download/axon-latest-linux-amd64
chmod +x axon && mv axon /usr/local/bin/

# Linux (arm64)
curl -Lo axon https://github.com/dexwritescode/axon/releases/latest/download/axon-latest-linux-arm64
chmod +x axon && mv axon /usr/local/bin/
```

Or build from source (requires Rust stable):

```bash
cargo install --path .
```

---

## Configuration

Axon reads `~/.config/axon/config.toml` on startup:

```toml
[backend]
base_url = "http://localhost:11434/v1"   # any OpenAI-compatible endpoint
model    = "qwen3:30b-a3b"

tool_approval = "ask"   # allow | ask | deny
```

---

## Usage

```bash
axon
```

Type your goal and press **Enter**. Use **Shift+Enter** for multi-line input. **Ctrl+C** exits.

---

## What Axon is not

- Not a GUI (that is the [Neurons](https://github.com/dexwritescode/neurons) desktop app)
- Not coupled to any specific model or provider
- Not a replacement for `nctl` (service management / model testing)
