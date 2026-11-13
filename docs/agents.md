# Using mcp-gate from an agent

Run mcp-gate without a terminal. The process must not wait for a menu. Pass
`--json` whenever you need a machine-readable result. Do not pass `--yes` unless
the user has already chosen the exact clients and servers.

## Exit codes

| Code | Meaning |
| --- | --- |
| 0 | The command finished. A canceled wizard also exits 0 and changes nothing. |
| 1 | The operation failed. With `--json`, stdout is one object whose `phase` is `failed`. |
| 2 | The arguments could not be parsed. With `--json` anywhere in the arguments, stdout is the same failure object and stderr is empty. |

`--help` and `--version` stay human text and exit 0.

## Inspect

```sh
mcp-gate status --json
```

`services` lists LaunchAgents named `local.mcp-gate.<name>`. `ownership` is
`managed` when this installation's registry records the agent, and `unmanaged`
when it does not. `state` is `running` when the gateway's health endpoint
answers. `connections` lists client MCP entries. Gateway health does not prove
that an application has switched to the gateway.

```sh
mcp-gate status --details
```

prints the same facts for a person, including source files.

## Preview and change

```sh
mcp-gate setup --dry-run --json
mcp-gate setup --client codex --server NAME --yes --json
mcp-gate remove NAME --dry-run
mcp-gate remove --all --yes --json
mcp-gate adopt NAME --dry-run
mcp-gate adopt NAME --json
```

`setup` does not accept `--all`. `remove NAME` is the same as `--server NAME`.
`adopt` only writes the local registry. It does not restart the gateway, edit
client settings, or copy the binary. `--dry-run` prints the plan and writes
nothing.

`check --config PATH` prints a one-line summary. `check --config PATH --json`
prints `{"format_version":1,"phase":"ok",...}`.

## Failure shape

```json
{"format_version":1,"phase":"failed","error":"why the command stopped"}
```

The `error` string is plain text. It does not contain gateway tokens. Read it,
fix the input, and run the command again. Do not retry a command that may
already have changed client settings; inspect `status --json` first.
