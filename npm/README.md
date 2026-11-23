# mcp-gate

Local Rust gateways reuse compatible MCP servers and start backends on demand.

Release targets include macOS ARM64 and Intel, Windows x64 and ARM64, and Linux
x64 and ARM64 with glibc 2.35 or newer or static musl. The launcher selects the
binary for your system from its optional dependencies. [Verified compatibility matrix](https://github.com/Matvey-Radchenko/mcp-gate/blob/main/docs/compatibility.md)

```sh
npx @matvei-radchenko/mcp-gate@latest setup
npx @matvei-radchenko/mcp-gate@latest status
npx @matvei-radchenko/mcp-gate@latest remove
npm i -g @matvei-radchenko/mcp-gate
```

The command name stays `mcp-gate` for Codex, OpenCode, and Claude Code.

Use `setup --dry-run` to preview settings without running MCP commands. Connections
without a working directory stay in that preview; pass `--cwd <dir>` or use the
interactive prompt. Select project settings with `--project /path/to/project` when
Claude Code has a project-scoped server. Shared project files are never rewritten.
Setup skips entries without a verified local override.

Setup registers user services and copies the Rust binary to permanent private
storage. The npm launcher itself does not change client settings or services.
Linux service setup requires `systemd --user`. On systems such as Alpine without
it, you can run a configured gateway manually with `mcp-gate serve --config <path>`.
MCP packages and their dependencies remain managed by you. Initial discovery
runs the existing command, which can download dependencies through npx, uvx,
or Docker. Restart each affected client once after setup or remove.

`status` does not start backends; `status --probe` explicitly requests discovery.
`remove` restores direct connections without deleting MCP packages or data.
Updates and compatible downgrades are applied by running setup with the chosen
mcp-gate version. Sessions and actions are not restored or replayed after restart.
