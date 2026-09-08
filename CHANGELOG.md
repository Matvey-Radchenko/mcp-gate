# Changelog

## 0.1.0 — unreleased

- Public `mcp-gate` command with setup, status and remove.
- Lazy stdio backends behind authenticated local Streamable HTTP.
- Shared/session ownership, bounded queues, cancellation and catalog drift checks.
- Explicit backend working directory, portable process ownership and private files.
- Personal client adapters, private transaction journals and safe deferred service cleanup.
- Permanent native binaries with user LaunchAgents and Windows logon tasks.
- Native GitHub CI matrix and npm launcher/platform archives.

Public release remains gated on the [compatibility checks](docs/compatibility.md).
Existing private `mcp-session-gateway` installations retain their historical names
and are not automatically adopted by setup.
