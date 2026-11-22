# Profiles

A profile is a TOML file that changes the agent. Point `AXON_PROFILE` at it:

```sh
AXON_PROFILE=buddy.toml axon
```

For example:

```toml
system_prompt = """
You are BUDDY, a voice assistant. Answer in one or two short sentences.
"""
tools = ["read_file", "grep"]
allow = ["get_time"]
```

## Fields

Every field is optional.

| Field | Meaning |
| --- | --- |
| `system_prompt` | Replaces Axon's own system prompt |
| `tools` | The built-in tools the model gets. Leave it out for all of them; `[]` gives none |
| `allow` | MCP tools that run without asking. Every other MCP tool asks first |

The built-in tools are `read_file`, `list_dir`, `grep`, `find`, `write_file`, `edit_file` and `run_command`.

MCP tools come from the servers an editor sends over [ACP](acp.md).

## Errors

Axon stops with an error if the profile can't be read or parsed, or names an unknown field or built-in tool.

MCP tool names are known only once their servers start. So an `allow` entry that matches no tool is reported as an error when a session with MCP servers starts, not when Axon starts.
