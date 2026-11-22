# Axon

[![Build](https://github.com/dexwritescode/axon/actions/workflows/build.yml/badge.svg?branch=main)](https://github.com/dexwritescode/axon/actions/workflows/build.yml)
[![Test](https://github.com/dexwritescode/axon/actions/workflows/test.yml/badge.svg?branch=main)](https://github.com/dexwritescode/axon/actions/workflows/test.yml)

Axon is an AI coding agent for local models. It runs as a terminal UI or as an [ACP](https://agentclientprotocol.com/) agent for editors such as Zed.

## Quick start

Axon talks to any OpenAI-compatible server. By default it uses LM Studio on `localhost:1234` with Qwen3.6 35B A3B.

Clone the repo, start the server, then run Axon from the repo's root folder:

```sh
git clone https://github.com/dexwritescode/axon.git
cd axon
lms server start                       # start LM Studio's server
lms load qwen/qwen3.6-35b-a3b          # load the model
cargo run --bin axon                   # build and run Axon
lms unload --all && lms server stop    # when done: unload the model and stop the server
```

Type a prompt and press Enter. Type `/exit` to quit.

## Docs

- [Terminal UI](docs/tui.md): keys, queued prompts, sessions
- [Configuration](docs/configuration.md): servers, models and other settings
- [Profiles](docs/profiles.md): custom system prompt and tools
- [ACP](docs/acp.md): Zed and other editors, MCP servers
- [Evals](docs/evals.md): measuring Axon on coding tasks
- [Development](docs/development.md): backend tests and releases

## License

Axon is licensed under either of

- [Apache License, Version 2.0](LICENSE-APACHE)
- [MIT license](LICENSE-MIT)

at your option.

Unless you explicitly state otherwise, any contribution intentionally submitted for inclusion in Axon by you, as defined in the Apache-2.0 license, shall be dual licensed as above, without any additional terms or conditions.
