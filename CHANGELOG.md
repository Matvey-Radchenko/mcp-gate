# Changelog

## Unreleased

- README rewritten.
- Session-owned backends close after 15 minutes without a request (`session_idle_seconds`) and release the worker. A backend process that exits releases its slot immediately. When every slot is busy, the longest-idle session idle for at least 2 minutes (`session_idle_evict_seconds`) is stopped so a new call can start.
- The Windows release download check passes the archive name to tar relative to its directory. Git Bash tar treated `C:\...` as a remote host.
- The test client reads an SSE response through the end of its HTTP body before the next request. A partial read on Windows could make the following response empty.

## 0.1.0 — unreleased

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
