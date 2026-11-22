# ACP

`axon acp` runs Axon as an [Agent Client Protocol](https://agentclientprotocol.com/) agent over stdio, so editors such as Zed can use it. It reads the same [configuration](configuration.md) and [profile](profiles.md) as the terminal UI.

## Zed

Zed starts `axon` itself, so install it on your PATH first with `cargo install --path crates/axon`. Then add it to Zed's settings:

```json
"agent_servers": { "Axon": { "command": "axon", "args": ["acp"] } }
```

## What it supports

- `session/new`, `session/prompt` (text only) and `session/cancel`
- Streamed replies, reasoning and tool calls
- Approval through `session/request_permission` for file changes (with a diff), commands and MCP tools

A session runs one prompt at a time. A prompt sent while another runs is an error.

## MCP servers

Axon starts the stdio MCP servers the editor sends with `session/new`, in the session's working directory, and stops them when the session ends. Their tools are offered next to the built-in ones, under their own names.

- A tool with the same name as one already offered is skipped.
- A server that fails to start, or a skipped tool, is reported as an error message. The session goes on without it.
- A call runs until it answers, the turn is cancelled, or 60 s pass. The model gets the call's text content.
- HTTP and SSE servers are ignored, with a note on stderr.

MCP tools ask for approval before they run, unless a [profile](profiles.md) lists them in `allow`.
