This release adds shared/session ownership controls and new release targets.
`mcp-gate share <name>` and `mcp-gate isolate <name>` set a gateway's ownership
explicitly, with advice from the saved catalog. Shared backends can opt into
parallel calls with `max_concurrent_calls`. `mcp-gate connect` lets stdio MCP
clients, including Claude Desktop on macOS and Windows, use an existing local
gateway. Windows ARM64 and static musl builds for Linux x64 and ARM64 are new;
the npm launcher selects the binary for the host architecture and Linux libc.
Backends that declare MCP extensions, such as `@upstash/context7-mcp` 4.8.0
with `io.modelcontextprotocol/skills`, now work through their core tools,
resources and prompts instead of failing setup.

Upgrading: run `mcp-gate setup` once after installing 0.2.0. It migrates
selected adopted and legacy gateway services to the managed binary store and
moves Codex and Claude Code headers helpers to a stable executable path. Then
restart the clients it lists. `mcp-gate setup --dry-run` previews the change.

Install with `npx @matvei-radchenko/mcp-gate@latest setup` or
`npm i -g @matvei-radchenko/mcp-gate`. The command remains `mcp-gate`.
Release binaries are built for macOS ARM64 and Intel, Windows x64 and ARM64,
and Linux x64 and ARM64. The glibc builds require glibc 2.35 or newer. The
musl builds work on distributions such as Alpine, where a configured gateway
can run with `mcp-gate serve --config <path>` when `systemd --user` is unavailable.
Native login acceptance is still the 0.1.0
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
workflow-driven publish. The Windows ARM64 and two Linux musl platform packages
are created by a one-time owner publish. See `docs/releasing.md`.
