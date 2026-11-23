# Using mcp-gate from an agent

Run mcp-gate without a terminal. The process must not wait for a menu. Pass
`--json` whenever you need a machine-readable result. It is a global flag, so
both `mcp-gate --json status` and `mcp-gate status --json` work. Do not pass
`--yes` unless the user has already chosen the exact clients and servers.

## Exit codes

| Code | Meaning |
| --- | --- |
| 0 | The command finished. A canceled wizard also exits 0 and changes nothing. |
| 1 | The operation failed. With `--json`, stdout is one object whose `phase` is `failed`. The same plain error is also written to stderr. |
| 2 | The arguments could not be parsed. With `--json` anywhere in the arguments, stdout is one JSON object and stderr is empty. `error` is a single line, such as `unrecognized subcommand 'bogus'`. `usage` and `hint` may also be present. |

`--help` and `--version` stay human text and exit 0.

## Inspect

```sh
mcp-gate status --json
```

`status --json` is one compact JSON object, not pretty-printed. `format_version`
stays 1. `services` lists LaunchAgents named `local.mcp-gate.<name>`. New
gateways use the server name; a 0.1.0 gateway may still be
`local.mcp-gate.<uuid>`. Both are managed the same way and are not renamed.
`ownership` is `managed` when this installation's registry records the agent,
and `unmanaged` when it does not. `state` is `running` when the gateway's health
endpoint answers. `connections` lists client MCP entries. `kind` is `direct`,
`http`, `managed`, or `external`. A direct connection's `state` is
`health not checked` and its `reason` says that backend health does not apply
until setup. `external` means the client already uses a gateway this
installation did not set up, so setup and remove do not own it. Gateway health
does not prove that an application has switched to the gateway. An explicit
`--server` or `--client` that matches nothing exits 1. With `--json`, `phase`
is `failed`.

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
nothing. Connections that have no working directory stay in the plan with
`needs_directory` and `directory_hint`. Pass `--cwd <dir>` to fill those in.
mcp-gate does not default that directory to the home directory; pass
`--cwd "$HOME"` only when the server does not depend on a project directory.
`service_label` is the LaunchAgent or scheduled-task name that would be
registered, such as `local.mcp-gate.filesystem`. Identical command, arguments,
environment and working directory share one label (`shares_gateway`). In a dry
run the autostart sentence says what *would* be registered.
`--diff` shows `command`, `args`, `cwd` and the gateway `url`. Argument values
that look like secrets, including `--token=...`, become `<hidden>`. Environment
values, headers and tokens stay hidden. `--yes` requires both `--client` and
`--server`. The exception is `remove --all --yes`.

`idle` names one managed gateway rather than selecting client settings:

```sh
mcp-gate idle NAME --json
mcp-gate idle NAME 15m --dry-run --json
mcp-gate idle NAME off --yes --json
```

Only use `--yes` after the user chooses that gateway and policy. Whole seconds and
integer `s`/`m`/`h` durations are accepted; `off` and `0` disable expiry. JSON reports
`idle_seconds` (0 when disabled), `previous_idle_seconds`, and `uses_ownership_default`.
Previews and saved changes include `restart_command`. The command saves only the
gateway config and never restarts its process. Obtain authorization for the printed
restart separately and wait for current actions to finish. See the
[idle contract](configurable-runtime.md#backend-idle-policy).

`check --config PATH` prints a one-line summary. `check --config PATH --json`
prints `{"format_version":1,"phase":"ok",...}`.

## Failure shape

```json
{"format_version":1,"phase":"failed","error":"why the command stopped"}
```

The `error` string is plain text. It does not contain gateway tokens. Read it,
fix the input, and run the command again. Do not retry a command that may
already have changed client settings; inspect `status --json` first.
