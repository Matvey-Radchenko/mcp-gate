# Compatibility and release gates

The npm 0.1.0 release is **not published**. This table is evidence, not a promise
that all combinations have been tested. Update it only from completed checks.

| Check | macOS ARM64 | macOS Intel | Windows x64 |
| --- | --- | --- | --- |
| Rust formatting, strict Clippy, size budgets, ordinary tests | Local passing; latest CI rerun pending | Earlier CI passed; startup deadline fix pending | Core passed; path-assertion fix pending |
| User service setup, repeat setup, remove | Passed locally and in earlier CI | Passed in earlier CI | Native service test pending |
| Recovery at five installation and three update stages | Passed locally | Pending | Pending |
| Remove one client while retaining another | Passed locally | Pending | Pending |
| Codex 0.153.4 shared/session tool calls | Passed locally and in earlier CI | Passed in earlier CI | Pending |
| OpenCode 1.14.23 private header and lazy discovery | Passed locally and in earlier CI | Passed in earlier CI | Pending |
| OpenCode 1.14.23 tool call with a local model | Passed locally | Pending | Pending |
| Claude Code 2.1.160 local model tool call and headers helper | Passed locally and in earlier CI | Passed in earlier CI | Pending |
| Claude local MCP precedence and physical project key | Passed with real CLI; shared file unchanged | Pending | Pending |
| Clean npm archive install | Passed locally and in earlier CI | Passed in earlier CI | Pending |
| Busy update deferred; idle update retains credentials | Passed locally | Pending | Pending |
| Autostart after an actual new login; compatible version downgrade | Pending | Pending | Pending |
| Chrome DevTools 1.8.0 browser independence and owned cleanup | Passed locally with Codex | Pending | Pending |
| Playwright 0.0.80 independent browsers, retained artifacts and cleanup | Passed locally with Codex | Pending | Pending |
| Real npx/uvx with local offline fixture packages | Passed locally | Pending | Pending |
| Windows Job Objects and native private-file ACLs | N/A | N/A | Passed in native ordinary tests |
| Docker container ownership and cleanup | Blocked: local daemon unavailable | Pending | Pending |

Earlier complete macOS CI evidence: [run 34264584770](https://github.com/Matvey-Radchenko/mcp-gate/actions/runs/34264584770), commit `e08454d`.
Later [run 34269320294](https://github.com/Matvey-Radchenko/mcp-gate/actions/runs/34269320294)
identified test startup deadlines and Windows path spelling assumptions. Its failure
is retained; it does not establish release readiness. Pending fixes require new CI.

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

Installer scope still needs final acceptance: ordinary global commands without an
explicit cwd are skipped. In particular, OpenCode configurations do not gain a
made-up cwd field; a supported project-local solution remains a separate setup.
The OpenCode transport tests above do not establish automatic migration for every
configuration source or schema.

Automatic recipes currently carry only reviewed `macos-aarch64` conditions; other
platforms do not select shared ownership from an untested recipe. Playwright's
narrow recipe explicitly changes backend cwd to its retained per-connection output
directory, and only accepts its listed startup flags with no custom environment.
