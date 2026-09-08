# mcp-gate

Local Rust gateways reuse compatible MCP servers and start backends on demand.

Supported release targets: macOS ARM64, macOS Intel, Windows x64. See the
[verified compatibility matrix](https://github.com/Matvey-Radchenko/mcp-gate/blob/main/docs/compatibility.md)
for the client versions and scenarios actually tested.

```sh
npx mcp-gate@latest setup
npx mcp-gate@latest status
npx mcp-gate@latest remove
```

Use `setup --dry-run` to preview settings without running MCP commands. Select
project settings with `--project /path/to/project`. Shared project files are
never rewritten. Setup skips entries without a verified local override.

Setup registers user services and copies the Rust binary to permanent private
storage. The npm launcher itself does not change client settings or services.
MCP packages and their dependencies remain managed by you. Initial discovery
runs the existing command, which can download dependencies through npx, uvx,
or Docker. Restart each affected client once after setup or remove.

`status` does not start backends; `status --probe` explicitly requests discovery.
`remove` restores direct connections without deleting MCP packages or data.
Updates and compatible downgrades are applied by running setup with the chosen
mcp-gate version. Sessions and actions are not restored or replayed after restart.
