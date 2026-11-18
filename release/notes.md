Patch 0.1.2 of mcp-gate: local Rust gateways for existing stdio MCP servers.
This release adds Linux x64 and ARM64 (glibc 2.35 or newer, systemd --user,
musl is not supported), unifies backend idle expiry as `idle_seconds` with
`mcp-gate idle`, lets the Windows npm launcher start from a long path, waits
for npm to serve the new `latest` dist-tag after publish, speeds up setup
digests, and fails fast when a restored Linux service exits on start.

Install with `npx @matvei-radchenko/mcp-gate@latest setup` or
`npm i -g @matvei-radchenko/mcp-gate`. The command remains `mcp-gate`.
Release binaries are built for macOS ARM64, macOS Intel, Windows x64,
Linux x64 and Linux ARM64. Linux requires glibc 2.35 or newer (Ubuntu 22.04,
Debian 12, Fedora 36, and later). Native login acceptance is still the 0.1.0
result: macOS ARM64 only. The Intel x86_64 run under Rosetta 2 is postponed, and
Windows x64 was smoke-tested under Wine rather than a native Windows login.
Stability on native Intel Macs and Windows is not guaranteed; reports are
welcome. Wine 7.7 does not present the protected non-null DACL the Windows build
requires; that is a limitation of running under Wine, not a Windows bug. See
`docs/compatibility.md` for the tested client versions and recipe constraints.

Setup registers user autostart services and preserves client permission settings.
MCP packages remain user-managed. Remove restores direct connections without
deleting packages or user data. Sessions and actions are never replayed after restart.

The attached SHA256SUMS covers the native archives attached to this Release.
npm packages are on the registry and are not Release assets. Environment and
owner-side npm trusted publisher configuration is required before a later
workflow-driven publish. The two Linux platform packages are created by a
one-time owner publish; see `docs/releasing.md`.
