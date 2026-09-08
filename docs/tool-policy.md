# Configured tool policy

Tool restrictions are configuration, not server-specific Rust branches. Defaults
preserve existing behavior. The raw catalog still pins **all** upstream tools, so
filtering cannot hide schema drift. `check` and `serve` validate policy names and
scoped argument types against that catalog; typoed restrictions fail closed.

```toml
[tool_policy]
disabled = ["configure_service"]
instructions = "Connection configuration is owned by the local operator."

[[tool_policy.scoped_directories]]
tool = "download_images"
argument = "localPath"
root = "/absolute/private/cache/images"
```

Disabled tools are omitted from frontend discovery and direct calls are rejected
before entering the worker queue. Backend initialization still discovers them.
The current health/CLI tool count is the full upstream catalog count, not the
filtered client-visible count.

For scoped arguments, the client supplies a relative directory. The gateway
rewrites it to `<root>/<server-generated MCP-session UUID>/<relative directory>`.
The same session reuses its namespace; new/reconnected MCP sessions get fresh
ones. A namespace is not a Codex task ID. Two clients can share one backend without
clobbering the same output path. Same-session writes to the same filename may
overwrite one another, following backend behavior.

Frontend property descriptions explain the relative-path contract. The original
catalog is not mutated. Missing/non-string paths, absolute paths, parent traversal,
backslashes, colons, NUL and preexisting symlink directory components are rejected
before starting the backend. The backend must separately validate nested filenames.

This is **not an OS sandbox**: the process runs with the user's privileges. It is
argument containment for trusted local servers/clients, not protection from a
hostile local user changing symlinks between validation and writing. Cache files
persist after session close; no age-based deletion or eviction is implemented.
Copy returned paths into the target project explicitly. Do not set the cache root
to the home directory or a repository collection.

## Legacy upstream stdio

The gateway additionally accepts upstream MCP `2024-11-05` for tools-only servers.
Resources, prompts and extensions on that legacy revision are rejected. The
frontend remains modern Streamable HTTP; it does not expose a 2024 HTTP transport.
This compatibility path was tested with the actual TestOps SDK 0.5.0 through native
Codex clients. It is not blanket support for every historical MCP capability.
