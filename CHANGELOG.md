# Changelog

## Unreleased

- Fixed: gateway HTTP listeners set `TCP_NODELAY` on accepted connections, so loopback Streamable HTTP calls no longer wait ~40 ms for a delayed ACK.
- `mcp-gate share <name>` and `mcp-gate isolate <name>` show shareability advice from the saved catalog and explicitly set a managed gateway's ownership. They support `--dry-run`, `--yes`, and `--json`, and print the command to apply the change by restarting the gateway. Unknown servers stay session-owned, `readOnlyHint` is not treated as evidence, and saving requires the gateway to run the same mcp-gate binary.
- Catalog refreshes preserve explicit ownership choices. Setup continues to match new clients by their proposed ownership mode.
- `cargo xtask bench` compares direct stdio, shared, and session modes with an offline mock backend.
- `mcp-gate connect <name> [--config <path>]` lets stdio MCP clients use an existing local gateway and closes its HTTP session when the client disconnects.
- Codex and Claude Code headers helpers use a stable executable path across upgrades. Re-run `mcp-gate setup` once to migrate managed and external helper paths from the binary store, then restart the clients it lists.
- Setup migrates selected adopted and legacy gateway services to the managed binary store. LaunchAgent, systemd user unit, and Windows logon task settings stay intact. Connections with no saved bindings stay external.
- Setup previews adopted gateway migrations with `--dry-run` and leaves already-current services running.

## 0.1.2 — 2026-11-18

- Unified backend idle expiry as `idle_seconds`: session-owned defaults to 900 seconds, shared defaults to off, and `0` disables either. Accept `"off"` on input and the legacy `session_idle_seconds` / `shared_idle_seconds` aliases; serialize only the new key and reject duplicate idle keys.
- Only executable requests extend either idle timer. Session-owned backends retain the open GET-stream exception for recordings; idle expiry still closes that HTTP session. Capacity eviction remains session-only and available with idle disabled. Shared Docker cannot enable idle expiry.
- `mcp-gate idle NAME [DURATION|off]` shows or saves the policy, supports `s`/`m`/`h`, `--dry-run`, `--yes` and `--json`, and prints the required service restart command without restarting it.
- Shared idle shutdown retains HTTP sessions, blocks on active/admitted queued work, and starts a fresh backend for the next executable request without hiding crashes or replaying actions.
- Linux x64 and ARM64: `systemd --user` units, XDG state under `$XDG_DATA_HOME/mcp-gate` (default `~/.local/share/mcp-gate`), and npm packages `@matvei-radchenko/mcp-gate-bin-linux-x64` and `@matvei-radchenko/mcp-gate-bin-linux-arm64`. Release binaries need glibc 2.35 or newer (Ubuntu 22.04, Debian 12, Fedora 36, and later); musl is not supported.
- The Windows npm launcher passes the resolved executable through `toNamespacedPath` so a long pnpm cache path can start the native binary.
- After publishing, the release script waits until npm serves the new `latest` dist-tag before checking it.
- Setup hashes the current executable in the background during the first prompt, streams the digest and the permanent binary install, and reuses that digest within the process.
- On Linux, setup recovery fails fast when a restored service exits or crash-loops, and clears systemd start-rate limits with `reset-failed`, instead of waiting out the full readiness window.

## 0.1.1 — 2026-11-15

- A timed-out shared call records the failure before the next waiter can take the worker, so that queued call is not dispatched.
- README visuals: wordmark, before/after diagram, and a setup recording.
- README rewritten.
- `setup --dry-run` keeps personal connections that have no working directory in the plan and tells you to run interactive setup or pass `--cwd <dir>`. No directory is assumed, including the home directory.
- Dry-run text says what would be registered. New gateways use a readable `local.mcp-gate.<name>` label, shown in the preview. Existing `local.mcp-gate.<uuid>` services stay on that label.
- Identical command, arguments, environment and working directory share one gateway. Session mode still isolates backends per connection.
- `--json` is global. Setup, remove, status and adopt describe their flags. `--yes` requires both `--client` and `--server`; `remove --all --yes` is the exception.
- Status names unchecked direct commands, drops the application-switch note until a gateway is managed, and returns an error for an unknown `--server` or `--client`. Running mcp-gate with no subcommand prints a short start-here message.
- Search typed between setup prompts is not echoed onto the following status line.
- `--diff` shows the command, arguments, working directory and gateway URL, and still hides environment values, headers and secret-like argument values.
- Session-owned backends close after 15 minutes without a request (`session_idle_seconds`) and release the worker. A backend process that exits releases its slot immediately. When every slot is busy, the longest-idle session idle for at least 2 minutes (`session_idle_evict_seconds`) is stopped so a new call can start.
- The Windows release download check passes the archive name to tar relative to its directory. Git Bash tar treated `C:\...` as a remote host.
- The test client reads an SSE response through the end of its HTTP body before the next request. A partial read on Windows could make the following response empty.

## 0.1.0 — 2026-11-14

- Public `mcp-gate` command with setup, status and remove.
- npm packages are `@matvei-radchenko/mcp-gate` and `@matvei-radchenko/mcp-gate-bin-{darwin-arm64,darwin-x64,win32-x64}`. The command name stays `mcp-gate`.
- Lazy stdio backends behind authenticated local Streamable HTTP.
- Shared/session ownership, bounded queues, cancellation and catalog drift checks.
- Explicit backend working directory, portable process ownership and private files.
- Personal client adapters, private transaction journals and safe deferred service cleanup.
- Permanent native binaries with user LaunchAgents and Windows logon tasks.
- Native GitHub CI matrix and npm launcher/platform archives.
- npm launcher version read from package.json, with homepage, bugs, keywords, and author on the meta and platform packages.
- Packing the meta package fails unless README and LICENSE are beside package.json.
- GitHub Release checksums list the attached native archives.
- Release process for current npm trusted publishing, plus an optional Homebrew tap formula.
- 0.1.0 ships with documented limits: native login acceptance is macOS ARM64 only; Intel under Rosetta 2 is postponed; Windows was smoke-tested under Wine.
- Human-readable setup, status, remove and check output, with JSON only when `--json` is set.
- `status` lists running `local.mcp-gate` LaunchAgents, and `adopt` records an unmanaged one.
- Agent instructions in `docs/agents.md` and `skills/mcp-gate/SKILL.md`.
- Connections through a gateway this installation did not set up are reported as `external`. `remove` names that situation instead of starting with "Nothing to remove".

Public release records those limits in the [compatibility checks](docs/compatibility.md).
Existing private `mcp-session-gateway` installations retain their historical names
and are not automatically adopted by setup.
