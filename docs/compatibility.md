# Compatibility and release gates

The npm 0.1.0 release is **not published**. This table is evidence, not a promise
that all combinations have been tested. Update it only from completed checks.

| Check | macOS ARM64 | macOS Intel | Windows x64 |
| --- | --- | --- | --- |
| Rust formatting, strict Clippy, size budgets, ordinary tests | Passed locally and in CI | Passed in CI | Passed in CI |
| User service setup, repeat setup, remove | Passed locally and in CI | Passed in CI | Passed in CI |
| Recovery at five installation and three update stages | Passed locally and in CI | Passed in CI | Passed in CI |
| Remove one client while retaining another | Passed locally and in CI | Passed in CI | Passed in CI |
| Codex 0.153.4 shared/session tool calls | Passed locally and in CI | Passed in CI | Passed in CI |
| OpenCode 1.14.23 private header and lazy discovery | Passed locally and in CI | Passed in CI | Passed in CI |
| OpenCode 1.14.23 tool call with a local model | Passed locally and in CI | Passed in CI | Passed in CI |
| Claude Code 2.1.160 local model tool call and headers helper | Passed locally and in CI | Passed in CI | Passed in CI |
| Claude local MCP precedence and physical project key | Passed with real CLI; shared file unchanged | Passed in CI | Passed in CI |
| Clean npm archive install | Passed locally and in CI | Passed in CI | Pending |
| Busy update deferred; idle update retains credentials | Passed locally and in CI | Passed in CI | Passed in CI |
| Unavailable old backend permits later repair | Passed locally and in CI | Passed in CI | Passed in CI |
| Actual update/downgrade between separately versioned release builds | Passed locally with unpublished compatibility fixture | Pending | Pending |
| Autostart after an actual new login | Pending | Pending | Pending |
| Background access to macOS protected folders | Separate OS permission needed; acceptance pending | Pending | N/A |
| Chrome DevTools 1.8.0 browser independence and owned cleanup | Passed locally and in CI with Codex | Passed in CI | Passed in CI |
| Playwright 0.0.80 independent browsers, retained artifacts and cleanup | Passed locally and in CI with Codex | Passed in CI | Pending |
| Real npx/uvx with local offline fixture packages | Passed locally and in CI | Passed in CI | Passed in CI |
| Windows Job Objects and native private-file ACLs | N/A | N/A | Passed in native ordinary tests |
| Docker container ownership and cleanup | Passed locally with Docker Engine 28.3.2 | Pending | Pending |

The repository became public on 2026-09-09; GitHub Actions now starts successfully.
[Run 34326027974](https://github.com/Matvey-Radchenko/mcp-gate/actions/runs/34326027974),
commit `89f0f8d`, passed all three pinned clients, offline npx/uvx and Chrome
DevTools on all platforms. Its macOS ARM64 job also passed Playwright and clean
archive installation. Intel Playwright exceeded the fixture client's cold-start
deadline; Windows Playwright lacked inherited standard Chrome installation paths.

Commit `01a19f2` restores those Windows environment paths and makes the fixture
client's deadline cover gateway startup, queueing and execution. In
[run 34328519829](https://github.com/Matvey-Radchenko/mcp-gate/actions/runs/34328519829),
both macOS platforms passed through archive installation. Windows reached real Playwright
navigation and isolation, then exceeded Playwright's own 5-second screenshot
deadline. Its fixture now explicitly uses `--timeout-action 30000`; this change
requires native verification and does not alter user MCP commands or timeouts.

The [native acceptance procedures](native-acceptance.md) describe the Docker and
two-version fixtures, and the outstanding actual-login and protected-folder gates.
The unpublished compatibility fixture is built from the same reviewed 0.1.0 tree;
it does not establish compatibility with any future release's changed formats.

macOS client tool approvals and operating-system privacy grants are separate.
A locally observed LaunchAgent stalled opening a test backend under Documents;
the same test passed with its generated fixture contained in a temporary directory.
Setup explains that protected-folder access may need separate macOS approval.
The installer does not grant that access, move real MCP packages, or modify TCC.

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
