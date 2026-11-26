# mcp-gate

Every agent session starts its own copy of every stdio MCP server you configured,
even when it only needs the tool list. mcp-gate puts a small local Rust gateway in
front of each server you already use in Codex, OpenCode, Claude Code, or Claude
Desktop. Backends start on the first tool call; a reviewed stateless server can
share one backend across all sessions.

```sh
npx @matvei-radchenko/mcp-gate@latest setup    # pick connections, review the preview
npx @matvei-radchenko/mcp-gate@latest status   # "ready; backend idle" until first use
npx @matvei-radchenko/mcp-gate@latest remove   # put the direct connections back
npm i -g @matvei-radchenko/mcp-gate            # or install every command globally
```

The command is `mcp-gate`; `@matvei-radchenko/mcp-gate` is only the package name.
Restart each affected app once after setup or remove.

**New in 0.2:** Claude Desktop on macOS and Windows through the `mcp-gate connect`
stdio bridge; `mcp-gate share` and `mcp-gate isolate` to choose shared or
per-session backends, with advice based on the saved tool list; opt-in parallel calls
with `max_concurrent_calls`; Windows ARM64 and static Linux musl builds.

**Upgrading from 0.1.x:** run `mcp-gate setup` once with 0.2.0, then restart the
clients it lists. Setup moves everything to one fixed location so later upgrades
no longer have to touch your client settings. `mcp-gate setup --dry-run` shows
what will change first.

Release targets are macOS ARM64 and Intel, Windows x64 and ARM64, and Linux x64
and ARM64 with glibc 2.35 or newer or static musl. The launcher selects the binary
for your system from its optional dependencies.

Setup registers user services and copies the Rust binary to permanent private
storage. The npm launcher itself does not change client settings or services.
Linux service setup requires `systemd --user`; without it, run a configured
gateway with `mcp-gate serve --config <path>`. Gateways listen on loopback only,
each with its own bearer token. MCP packages and their dependencies remain
yours to install and update. Sessions and actions are not restored or replayed
after a restart.

[README](https://github.com/Matvey-Radchenko/mcp-gate#readme) ·
[Install and command reference](https://github.com/Matvey-Radchenko/mcp-gate/blob/main/docs/usage.md) ·
[Compatibility](https://github.com/Matvey-Radchenko/mcp-gate/blob/main/docs/compatibility.md) ·
[Changelog](https://github.com/Matvey-Radchenko/mcp-gate/blob/main/CHANGELOG.md)
