# Changelog

## 0.1.0 — unreleased

- Public `mcp-gate` command with setup, status and remove.
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

Public release records those limits in the [compatibility checks](docs/compatibility.md).
Existing private `mcp-session-gateway` installations retain their historical names
and are not automatically adopted by setup.
