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
| Docker container ownership and cleanup | Passed locally with Docker Engine 28.3.2 | Passed in CI with Colima 0.10.3 / engine 29.5.2 | Pending |

The repository became public on 2026-09-09; GitHub Actions now starts successfully.
[Run 34333009537](https://github.com/Matvey-Radchenko/mcp-gate/actions/runs/34333009537),
commit `11d69b0`, passed all three native jobs, including real clients, browser
ownership, services, independently built release versions and archive installation.
Windows DLL inspection confirmed that the executable does not need a separate
Visual C++ redistributable. Windows Playwright uses the explicit fixture option
`--timeout-action 30000`; setup does not alter user commands or timeouts.
The Codex fixture waits for each thread's MCP startup event and connected runtime
status before checking inventory and connection counts.

[Run 34336206965](https://github.com/Matvey-Radchenko/mcp-gate/actions/runs/34336206965),
commit `a624d6e`, also passed all five jobs. It builds one common npm launcher and
installs those exact bytes on all three native platforms, exercises actual local
and global command links, and verifies all seven release archives together.
The downloaded archives passed file-list, native-architecture, binary-hash,
shared-launcher and known private-path/token signature inspection. This resolves
the different launcher tar permissions found in the earlier separate-platform
builds. See the [source and archive audit](../release/source-audit.md).

A subsequent repeat of the same implementation exposed one remaining test race:
[run 34338316908](https://github.com/Matvey-Radchenko/mcp-gate/actions/runs/34338316908)
passed Windows and Intel, but ARM64 counted a third session immediately after
Codex inventory. In [Codex 0.153.4's implementation](https://github.com/openai/codex/blob/rust-v0.153.4/codex-rs/codex-mcp/src/mcp/mod.rs#L468),
`mcpServerStatus/list` creates a separate connection set even with `threadId`, then
cancels its startup after collecting inventory. The test now waits within a bounded
deadline for these probes to close before counting persistent sessions or choosing
session IDs for browser cleanup. It still requires the exact session count and
zero workers, and repeatedly checks that discovery preserves the thread's session
and leaves none after client exit. This repeated-inventory check passed on all three
platforms in [run 34341463558](https://github.com/Matvey-Radchenko/mcp-gate/actions/runs/34341463558).
Both macOS jobs completed successfully; Windows exposed three other failures.

Windows worker cleanup now waits for the entire owned Job Object to empty before
reporting zero workers or permitting maintenance. Closing its handle alone could
report completion before all descendants exited: [job termination](https://learn.microsoft.com/en-us/windows/win32/api/jobapi2/nf-jobapi2-terminatejobobject)
uses the asynchronous [process termination](https://learn.microsoft.com/en-us/windows/win32/api/processthreadsapi/nf-processthreadsapi-terminateprocess)
contract. Failed cleanup retains ownership and a busy worker count, with an error;
no backend action is replayed. Browser checks record creation times on Windows so
PID reuse cannot turn an unrelated process into an alleged orphan.

The Claude local-model fixture explicitly sets `MCP_CONNECTION_NONBLOCKING=0`:
2.1.160 otherwise permits its first query before MCP tools are ready. This is a
[test fixture setting](https://code.claude.com/docs/en/env-vars), not an installer
change to user behavior. Completion now requires the actual fixture tool result.
The Codex fixture owns its Windows npm shell/Node/native process tree in a Job
Object, so forced fixture shutdown also cleans up descendants. Startup timeouts
include the RPC method and last lifecycle event. Its configuration preflight now
awaits the subprocess with a deadline, instead of blocking the async runtime while
another client is waiting for an RPC response. The exact cause of the preceding
Codex startup timeout remains unproven.
[Run 34344798587](https://github.com/Matvey-Radchenko/mcp-gate/actions/runs/34344798587),
commit `ad36a4a`, subsequently passed all five jobs, including these corrections on
all three native platforms and the combined release archives.

[Run 34347193008](https://github.com/Matvey-Radchenko/mcp-gate/actions/runs/34347193008)
passed the additional native Intel Docker job and both ARM64/Windows native suites.
Intel's ordinary tests exposed an early shutdown failure. Previously, the async
signal function installed handlers only on its first poll, after the HTTP task
could already return readiness. Both Unix shutdown handlers and the Windows
console handler are now installed before binding the listener. A regression sends
signals before polling the receiver; it and the local full harness passed. The
native matrix must be repeated for this production change.

The Windows Docker runner successfully booted WSL2 and installed the engine in the
pinned Ubuntu distribution, but its detached Linux daemon did not become reachable.
The CI fixture now retains a foreground, host-owned WSL engine invocation and
stops both the distribution's Docker service and socket first. This infrastructure
correction reached the engine and pulled the image in
[run 34349203746](https://github.com/Matvey-Radchenko/mcp-gate/actions/runs/34349203746),
but the endpoint disappeared after the owning CI step exited, before the gateway
fixture started. Preparation and the entire Docker fixture now share one owning
PowerShell process; this lifetime correction still requires native verification.

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
