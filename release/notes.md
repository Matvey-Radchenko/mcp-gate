First stable npm release of mcp-gate: local Rust gateways for existing stdio MCP servers.

Install with `npx mcp-gate@latest setup`. Supported release binaries target macOS
ARM64, macOS Intel and Windows x64. See `docs/compatibility.md` for the tested
client versions and recipe constraints.

Setup registers user autostart services and preserves client permission settings.
MCP packages remain user-managed. Remove restores direct connections without
deleting packages or user data. Sessions and actions are never replayed after restart.

The attached SHA256SUMS covers the native archives and npm packages produced by
this release run. Environment and owner-side npm trusted publisher configuration
must be completed before this workflow can publish.
