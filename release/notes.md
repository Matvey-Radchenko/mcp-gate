First stable npm release of mcp-gate: local Rust gateways for existing stdio MCP servers.

Install with `npx mcp-gate@latest setup`. Release binaries are built for macOS
ARM64, macOS Intel and Windows x64. Native login acceptance for 0.1.0 is macOS
ARM64 only. The Intel x86_64 run under Rosetta 2 is postponed, and Windows x64
was smoke-tested under Wine rather than a native Windows login. Stability on
native Intel Macs and Windows is not guaranteed; reports are welcome. Wine 7.7
does not present the protected non-null DACL the Windows build requires; that is
a limitation of running under Wine, not a Windows bug. See `docs/compatibility.md`
for the tested client versions and recipe constraints.

Setup registers user autostart services and preserves client permission settings.
MCP packages remain user-managed. Remove restores direct connections without
deleting packages or user data. Sessions and actions are never replayed after restart.

The attached SHA256SUMS covers the native archives attached to this Release.
npm packages are on the registry and are not Release assets. Environment and
owner-side npm trusted publisher configuration is required before a later
workflow-driven publish.
