# Compatibility and release gates

The npm 0.1.0 release is **not published**. This table is evidence, not a promise
that all combinations have been tested. Update it only from completed checks.

| Check | macOS ARM64 | macOS Intel | Windows x64 |
| --- | --- | --- | --- |
| Rust format, strict Clippy, size budgets, ordinary tests | Passing before latest acceptance additions; final rerun pending | Native CI pending | Native CI pending |
| Compilation of all targets/tests | Local native build | Pending | Cross-check passed before latest additions; native run pending |
| User service setup, repeat setup, remove | In progress | Pending | Pending |
| Codex 0.153.4 shared/session tool calls | In progress | Pending | Pending |
| OpenCode 1.14.23 private file header, lazy discovery | In progress | Pending | Pending |
| Claude Code 2.1.160 local model tool call and headers helper | In progress | Pending | Pending |
| Claude local MCP precedence | Verified with CLI in isolated settings; shared file unchanged | Pending | Pending |
| Clean npm archive install | Pending | Pending | Pending |
| Login autostart, idle upgrade, busy upgrade, compatible downgrade | Pending | Pending | Pending |
| Browser independence, artifacts, owned-process cleanup | Previous private pilots; public installer retest pending | Pending | Pending |

Definitions: `session` means an MCP connection. It does not promise separation
between OpenCode chats sharing that connection; `_meta.sessionID` routing is excluded.

Adapters preserve permission fields and unrelated settings. Codex uses TOML,
OpenCode uses quoted-key JSON/JSONC, and Claude uses JSON plus personal project
scope. Unresolved context, dynamic settings, unsupported project overrides,
organizational/plugin-owned sources and existing HTTP entries are not migrated
by guessing. No second MCP with renamed tools is created to bypass precedence.

The six declarative [recipes](../recipes/verified.json) carry reviewed versions,
launch conditions and policy exceptions. Browser packages need independent
browsers. API recipes pin reviewed artifacts where public versions are not enough;
those hashes do not establish every transitive package version. Telegram requires
the previous TDLib owner to release its files before discovery.

Release is blocked until the required tests in the agreed plan pass, native
artifacts are inspected, npm names and owner authorization are available, and
trusted publishing is configured. Post-publication registry/download installation
must also be checked before declaring release complete.

Reference contracts: [Claude scope precedence and headers helpers](https://code.claude.com/docs/en/mcp),
[GitHub native runner labels](https://docs.github.com/en/actions/reference/runners/github-hosted-runners),
[npm trusted publishing](https://docs.npmjs.com/trusted-publishers/).
