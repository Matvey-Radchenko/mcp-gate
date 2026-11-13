# mcp-gate

Local Rust gateways for existing stdio MCP servers. Discovery stays available
while backends start on demand. Verified stateless API servers can share a
single serialized backend; session-owned servers keep independent MCP connections.
Actions are never retried or replayed automatically.

**0.1.0 is in release preparation and has not been published.** Native login
acceptance is macOS ARM64 only. The Intel x86_64 check under Rosetta 2 has not
been run; Windows x64 was smoke-tested under Wine, not on a native Windows login.
Stability on native Intel Macs and Windows is not guaranteed, and reports are
welcome. See the [compatibility and release gates](docs/compatibility.md).

In a terminal, `setup` and `remove` ask you to choose connections and show a
preview before changing anything. Arrows move, Space selects, Enter continues,
and Esc goes back. Nothing is selected automatically. Non-interactive use never
prompts: pass `--json` for one JSON document on stdout, and `--dry-run` to
preview. Applying changes from an agent requires explicit `--client` and
`--server` with `--yes`, or `remove --all --yes`. `status` lists client
connections and `local.mcp-gate.<name>` LaunchAgents. An unmanaged agent can be
recorded with `adopt <name>`; `adopt <name> --dry-run` changes nothing.
`--details` explains sources. Argument errors are JSON when `--json` is present.
Agents should follow [docs/agents.md](docs/agents.md). A reusable skill is in
[skills/mcp-gate/SKILL.md](skills/mcp-gate/SKILL.md).

```sh
npx mcp-gate@latest setup
npx mcp-gate@latest status
npx mcp-gate@latest remove
```

Release targets are macOS ARM64, macOS Intel and Windows x64. Linux is excluded.
A global npm install or a GitHub Release binary provides the same three commands.
The npm package only launches the Rust executable: installing or updating the
package does not switch existing connections.

`setup --dry-run` previews selected settings without launching a backend or writing
files. `--client`, `--server` and `--project` narrow the selection. `--yes` requires
both an explicit client and server. `--diff` shows a structural diff with values
hidden. `--json` produces a versioned result on stdout; diagnostics go to stderr.

Setup preserves names and client permission settings, records private recovery
information, prepares the catalog, registers and starts user services, checks
readiness, and only then changes personal client connections. Each affected
application needs one restart. A healthy gateway alone does not establish that
a running application has switched to it.

Project files intended for Git are left untouched. Claude local scope can override
a selected project through personal settings. Other project entries without a
verified local override are skipped. Commands with unresolved working directories
or environment expansion are also skipped with a reason.

Setup copies the Rust binary into permanent versioned storage, independently of
npx caches. macOS uses a LaunchAgent; Windows uses a user Task Scheduler logon task.
The gateways run after login and MCP backends remain lazy. State lives in
`~/Library/Application Support/mcp-gate` or `%LOCALAPPDATA%\mcp-gate`.
Sessions and unfinished actions are not restored after restart.

You retain responsibility for installing and updating MCP packages, interpreters,
Docker and browsers. Setup's discovery executes the existing command: npx, uvx
or Docker may download their own dependencies. Arbitrary shell dependency trees
are not analyzed or copied. Reviewed recipes use narrow version and invocation
conditions; names alone never select shared mode.

`status` lists connections and LaunchAgents. A connection kind of `external` means the client already uses a gateway this installation did not set up, so setup and remove do not own it. `status --json` prints one compact JSON object. `status --probe` explicitly requests
backend initialization when the gateway can safely enter maintenance and has no
existing backend. Busy gateways are deferred. `remove` restores direct connections
without deleting MCP packages, user data, logs, browser artifacts or old releases.

Run the chosen new or older mcp-gate version's `setup` to update compatible
background services. An idle service enters maintenance before replacement;
busy services wait for a later setup. Unsupported state formats are rejected.
Operation journals preserve recoverable changes without overwriting later edits.

Development: [CONTRIBUTING.md](CONTRIBUTING.md), [architecture](docs/architecture.md),
[low-level configuration](docs/configurable-runtime.md), [tool policy](docs/tool-policy.md).

```sh
cargo xtask check
```

This runs formatting, strict Clippy, source-size budgets and ordinary fixture/unit
tests. GitHub CI runs native checks on all three release platforms and verifies
clean npm archive installation. Client/service/browser acceptance is tracked
separately; a workflow definition is not evidence that its runs passed.

MIT licensed. Old `mcp-session-gateway` configurations remain readable. Connections
that already route through a gateway this installation did not set up are reported
as `external` and are not adopted or renamed automatically.
