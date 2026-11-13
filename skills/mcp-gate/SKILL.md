---
name: mcp-gate
description: Inspect, add, remove, and adopt local mcp-gate MCP gateways without a terminal prompt. Use when an agent installs or troubleshoots mcp-gate, or changes Codex, Claude Code, or OpenCode MCP connections.
---

# mcp-gate

Drive the `mcp-gate` binary. Do not open an interactive wizard. Full exit codes
and JSON fields are in `docs/agents.md`.

## Rules

- Pass `--json` for every command whose result you will parse. Stdout is one JSON document.
- A missing TTY must not block. If a command says a terminal is required, rerun it with `--dry-run` or with explicit `--client`, `--server`, and `--yes`.
- Never invent a server name. Read `services` and `connections` from `mcp-gate status --json` first.
- `adopt` records an existing `local.mcp-gate.<name>` LaunchAgent. Preview with `mcp-gate adopt NAME --dry-run` before `mcp-gate adopt NAME --json`.
- `setup` has no `--all`. Apply only with `mcp-gate setup --client CLIENT --server NAME --yes --json`.
- `remove NAME --dry-run` previews one connection. `remove --all --yes --json` restores every managed connection in scope.
- Exit 0 means the command finished. Exit 1 is an operation failure. Exit 2 is bad arguments. Both failures with `--json` use `{"format_version":1,"phase":"failed","error":"..."}`.
- Gateway health does not mean the user's application has reconnected. Say that when you report status.

## Examples

```sh
mcp-gate status --json
mcp-gate adopt NAME --dry-run
mcp-gate remove NAME --dry-run
mcp-gate setup --client codex --server NAME --dry-run --json
```
