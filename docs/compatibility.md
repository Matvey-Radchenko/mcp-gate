# Compatibility and release gates

The npm 0.1.0 release is **not published**. This table is evidence, not a promise
that all combinations have been tested. Update it only from completed checks.
CI uses macOS 15 and Windows Server 2025. Local ARM64 checks use macOS 26.6.2;
this evidence does not substitute for desktop Windows login acceptance.

| Check | macOS ARM64 | macOS Intel | Windows x64 |
| --- | --- | --- | --- |
| Rust formatting, strict Clippy, size budgets, ordinary tests | Passed locally and in CI | Passed in CI | Passed in CI |
| User service setup, repeat setup, remove | Passed locally and in CI | Passed in CI | Passed in CI |
| Recovery at five installation and three update stages | Passed locally and in CI | Passed in CI | Passed in CI |
| Remove one client while retaining another | Passed locally and in CI | Passed in CI | Passed in CI |
| Codex 0.153.4 shared/session tool calls | Passed locally and in CI | Passed in CI | Passed in CI with owned client cleanup and asynchronous preflight |
| OpenCode 1.14.23 private header and lazy discovery | Passed locally and in CI | Passed in CI | Passed in CI |
| OpenCode 1.14.23 tool call with a local model | Passed locally and in CI | Passed in CI | Passed in CI |
| Claude Code 2.1.160 local model tool call and headers helper | Passed locally and in CI | Passed in CI | Passed in CI with explicit fixture startup wait |
| Claude local MCP precedence and physical project key | Passed with real CLI; shared file unchanged | Passed in CI | Passed in CI |
| Clean npm archive install | Passed locally and in CI | Passed in CI | Passed in CI |
| Busy update deferred; idle update retains credentials | Passed locally and in CI | Passed in CI | Passed in CI |
| Unavailable old backend permits later repair | Passed locally and in CI | Passed in CI | Passed in CI |
| Actual update/downgrade between separately versioned release builds | Passed locally and in CI with unpublished compatibility fixture | Passed in CI | Passed in CI |
| Autostart after an actual new login | Pending | Pending | Pending |
| Background access to macOS protected folders | Separate OS permission needed; acceptance pending | Pending | N/A |
| Chrome DevTools 1.8.0 browser independence and owned cleanup | Passed locally and in CI with Codex | Passed in CI | Passed in CI |
| Playwright 0.0.80 independent browsers, retained artifacts and cleanup | Passed locally and in CI with Codex | Passed in CI | Passed in CI with full job drain and process identity checks |
| Real npx/uvx with local offline fixture packages | Passed locally and in CI | Passed in CI | Passed in CI |
| Windows Job Objects and native private-file ACLs | N/A | N/A | Passed in native ordinary tests |
| Docker container ownership and cleanup | Passed locally with Docker Engine 28.3.2 | Passed in CI with Colima 0.10.3 / engine 29.5.2 | Passed in CI with native CLI 29.1.5 / WSL engine 29.1.3 |

Completed native matrix evidence is recorded in
[release/acceptance.json](../release/acceptance.json).
[Run 34351749071](https://github.com/Matvey-Radchenko/mcp-gate/actions/runs/34351749071),
commit `66cb8e9`, passed all seven jobs: three native platforms, Intel/Windows
Docker, the common-launcher build and the combined archive check. All three platforms installed the exact same launcher archive and exercised
actual local/global commands. Native architectures, binary hashes, archive contents
and known private-path/token signatures were inspected; see the
[source and archive audit](../release/source-audit.md).

Docker acceptance passed separately on
[Intel](https://github.com/Matvey-Radchenko/mcp-gate/actions/runs/34347193008) and
[Windows](https://github.com/Matvey-Radchenko/mcp-gate/actions/runs/34351749071), in
addition to the local ARM64 check. The Windows job uses native `docker.exe`, WSL
2.7.12 and a checksum-pinned Ubuntu 24.04.4 engine, with literal Node arguments.
It does not test Docker Desktop installation or Windows drive-mount translation.
The version probe holds stdin open, matching MCP and avoiding the documented
[Docker/WSL half-close output limitation](https://github.com/docker/cli/issues/6220).

The tested client/browser scenarios have these explicit conditions:

- Codex fixtures wait for each thread's MCP startup and connected status. Its
  [inventory operation](https://github.com/openai/codex/blob/rust-v0.153.4/codex-rs/codex-mcp/src/mcp/mod.rs#L468)
  creates temporary discovery connections; tests require their bounded teardown
  before counting persistent connections, with zero backend workers during discovery.
- Claude's local-model fixture sets `MCP_CONNECTION_NONBLOCKING=0` to wait for MCP
  startup before the first query, and requires an actual successful tool result.
  This [fixture setting](https://code.claude.com/docs/en/env-vars) does not change
  users' client configuration.
- Windows Playwright uses `--timeout-action 30000`. Tests distinguish process
  identities from reused PIDs. The gateway waits for its entire Windows Job Object
  to empty before reporting idle or permitting maintenance; failed cleanup retains
  ownership and reports an error.
- Windows releases use static CRT linkage; their inspected DLL imports do not
  require a separate Visual C++ redistributable.

Shutdown handlers are installed before listener readiness. The regression sends
signals before polling the receiver; the local harness and real-client/browser
checks passed, followed by the complete seven-job native run linked above.

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
