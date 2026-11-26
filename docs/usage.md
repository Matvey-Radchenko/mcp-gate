# Install, upgrade and command reference

The [README](../README.md) has the quick start. This page has the details.

## Install

The command is `mcp-gate`; `@matvei-radchenko/mcp-gate` is only the npm package name. Installing or updating the package does not switch existing connections by itself: run `setup` from the build you want (an older build downgrades compatibly).

Release targets are **macOS ARM64 and Intel**, **Windows x64 and ARM64**, and **Linux x64 and ARM64**. Linux has glibc builds for glibc 2.35 or newer (Ubuntu 22.04, Debian 12, Fedora 36, and later) and static musl builds for distributions such as Alpine.

### npm

```sh
npx @matvei-radchenko/mcp-gate@latest setup
npx @matvei-radchenko/mcp-gate@latest status
npx @matvei-radchenko/mcp-gate@latest remove
```

Or install the launcher globally. It provides every command, including `adopt`, `idle`, `share`, `isolate`, and `connect`:

```sh
npm i -g @matvei-radchenko/mcp-gate
mcp-gate setup
```

The `@matvei-radchenko/mcp-gate` launcher selects the matching `@matvei-radchenko/mcp-gate-bin-<target>` optional dependency. On Linux it uses the musl package on musl systems and the glibc package on glibc systems. Each Linux package declares its libc, so npm installs only one of them.

### GitHub Release binary

Download `mcp-gate-<version>-<target>.tar.gz` for your platform and verify it against the attached `SHA256SUMS`. Put `mcp-gate` on your `PATH`, then run `mcp-gate setup`. [GitHub Releases](https://github.com/Matvey-Radchenko/mcp-gate/releases)

| Platform | Archive target suffix |
| --- | --- |
| macOS | `darwin-arm64`, `darwin-x64` |
| Windows | `win32-x64`, `win32-arm64` |
| Linux with glibc | `linux-x64`, `linux-arm64` |
| Linux with musl | `linux-x64-musl`, `linux-arm64-musl` |

## State and services

State lives in `~/Library/Application Support/mcp-gate`, `%LOCALAPPDATA%\mcp-gate`, or `~/.local/share/mcp-gate` (`$XDG_DATA_HOME/mcp-gate`). Gateways start after login; MCP backends stay lazy.

On Linux, setup registers a `systemd --user` unit under `~/.config/systemd/user` (or `$XDG_CONFIG_HOME/systemd/user`), with `WantedBy=default.target`. Units start with your user session after login. To keep the user manager — and those units — available after logout and at boot, run `loginctl enable-linger "$USER"`. If `systemd --user` is not reachable, setup leaves client configs unchanged and prints a manual fallback: prepare a private state directory and a config with absolute paths, then run `mcp-gate serve --config "/absolute/path/config.toml"`.

## Updating and downgrading

Run `setup` again with a newer or older build to update compatible background services. An idle service enters maintenance before replacement; a busy service waits until a later setup. Unsupported state formats are rejected. Operation journals keep recoverable changes and do not overwrite later edits.

`mcp-gate --version` prints the version. Builds from a git checkout add the commit, for example `0.2.0 (2dce20137)`; health and `status --json` report it as `commit`. Packaged release builds print the plain version. Include this line in bug reports.

### Upgrading from 0.1.x

Run `mcp-gate setup` once with 0.2.0, then restart the clients it lists. `mcp-gate setup --dry-run` shows what will change first. Setup moves everything to one fixed location, so later upgrades no longer have to touch your client settings:

- Codex and Claude Code read each gateway's bearer token through a small helper command. Setup points that command at `current/mcp-gate` (`current\mcp-gate.exe` on Windows) instead of a versioned path, including on connections setup did not create. Later setup runs update that executable in place. OpenCode keeps reading the token from a file.
- Gateway services you registered by hand with `mcp-gate adopt`, or that an older release created, move to mcp-gate's own versioned binary folder. Their LaunchAgent, systemd user unit, or Windows logon task settings stay the same. Connections with no saved settings stay `external`.

### Taking over a gateway you set up by hand

For a legacy user service, run `mcp-gate adopt <name>` once to record its identity. Then run `mcp-gate setup` with the official executable and select the adopted gateway. Use `setup --dry-run --json` to preview the migration. Setup points the service at `binaries/<version>-<sha256>/` and preserves its label, endpoint, credentials, backend, and service settings. Adoption saves no client bindings, so those connections stay `external`. Restart the client if setup migrated its headers helper. A subsequent setup leaves a gateway on the current managed binary running.

For unattended setup, select the external connection with the usual `--client`, `--server`, and `--yes` options. Setup matches the service by its endpoint. If the gateway has no matching client entry, use the adopted gateway name or service label as the `--server` value and supply `--client`.

## Setup and remove options

- `--dry-run` previews the selected changes without starting a backend or writing files.
- `--client`, `--server`, and `--project` narrow the selection. `--project` matters when Claude Code has a project-scoped server.
- `--cwd <dir>` sets the working directory for selected connections that do not declare one. Without it, interactive setup asks; mcp-gate never guesses. For a global `npx` or `uvx` server that does not use project files, `--cwd "$HOME"` is a fine explicit choice.
- `--diff` shows the command, arguments, working directory, and gateway URL. Environment values, headers, and anything secret-like stay hidden.
- `--yes` skips prompts and requires an explicit client and server, except `remove --all --yes`.
- `--json` works before or after the subcommand and writes one versioned document to stdout. Diagnostics go to stderr.

## Status

- `status --details` explains where each connection comes from. A direct connection shows `health not checked` until it has a gateway.
- `status --probe` asks a gateway to start its backend only when it has none and can safely pause; busy gateways are skipped.
- A `--server` or `--client` that matches nothing is an error.
- `external` means the client uses a gateway without saved direct settings. Setup can migrate a headers helper from this installation's binary store. Remove has no saved connection to restore.
- A healthy gateway does not prove that a running app has switched to it. Restart each affected app once after `setup`.

## Idle backends and capacity

`idle_seconds` defaults to 900 for per-session backends and off for shared ones; `0` disables it. Only executable requests (`tools/call`, `resources/read`, `prompts/get`) extend the timer. An open GET stream also keeps a per-session backend alive, for example while a browser recording runs. Shared backends, except Docker ones, can opt in while keeping their HTTP sessions open. When every worker slot is busy, `session_idle_evict_seconds` (default 120) lets the longest-idle per-session backend make room. Running calls are never evicted. See [configurable runtime](configurable-runtime.md#backend-idle-policy).

## Labels and shared gateways

- New gateways get a readable service label, `local.mcp-gate.<name>`, or the next free `-2`, `-3`, … if it is taken. The preview shows the label before anything is registered.
- Installations from 0.1.0 keep their `local.mcp-gate.<uuid>` labels. `status`, `setup`, and `remove` still manage them, and they are not renamed.
- The same command, arguments, environment, and working directory in several clients share one gateway. Session mode still starts a separate backend per MCP connection, and a session-mode server is never attached to a shared-mode gateway.

Claude Code local scope can override a selected project through personal settings. Other project entries without a verified local override are skipped, as are commands whose environment cannot be expanded.

## Non-interactive and agent use

Non-interactive use never prompts. Pass `--json` for one JSON document on stdout, and `--dry-run` to preview. Setup/remove changes require explicit `--client` and `--server` with `--yes`, or `remove --all --yes`. `idle <name> <duration> --yes` saves only that gateway's idle policy. Argument errors are JSON when `--json` is present. Follow [agents.md](agents.md).

```sh
mcp-gate status --json
mcp-gate setup --dry-run --json
mcp-gate setup --client codex --server NAME --yes --json
mcp-gate remove NAME --dry-run
mcp-gate adopt NAME --dry-run
```

Do not pass `--yes` unless the user has already chosen the exact clients and servers. If a command may already have changed client settings, inspect `status --json` before running it again.

Configurations from the project's earlier name, `mcp-session-gateway`, remain readable.
