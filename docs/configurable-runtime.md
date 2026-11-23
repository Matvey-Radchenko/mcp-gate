# Configurable runtime — unreleased

One executable, one gateway process per configured MCP, one authenticated loopback
HTTP endpoint per gateway. Clients connect directly; no per-client stdio adapters.
No live service deployment or client configuration changes are part of development.

## Ownership and request lifecycle

- `session` (default): each HTTP MCP session owns its lazy backend, including its
  roots and logging destination. Closing it does not close another session's backend.
- `shared` (explicit): the gateway owns one lazy backend across clients. Closing all
  clients does not shut it down. Gateway shutdown or an explicit backend idle
  timeout reaps it.
- The same command, arguments, environment and working directory use one gateway
  process even when two clients configure it. Session mode still gives each HTTP
  session its own backend. Shared mode still has one backend. The two ownership
  modes are not mixed in one gateway.
- Both modes default to one executable request (`tools/call`, `resources/read`,
  `prompts/get`) at a time per backend. `max_concurrent_calls` accepts integers
  from 1 to 64. Values greater than 1 require `ownership = "shared"`. The
  admission limit is `max_concurrent_calls + max_pending_calls`, and
  `queue_timeout_seconds` bounds waiting to take an execution slot. Requests
  beyond the admission limit fail immediately; zero pending calls means
  fail-fast while busy. Cancelled or expired queued calls never reach the
  backend. These limits are not OS memory quotas.
- If the backend process exits, including when its browser is gone and the
  process ends, the worker slot is released without waiting for another call on
  that same session. The session must reconnect; the action is not replayed.
- When every worker slot is busy, the longest-idle session-owned backend that
  has been idle for at least `session_idle_evict_seconds` (default 2 minutes;
  zero means any idle one) is shut down so the new call can start. A call that
  is still running is not evicted. This capacity policy is session-only, remains
  available with idle expiry disabled, and ignores the open GET stream exception.
  Existing disconnect grace still closes HTTP sessions after a lost GET when no
  executable call is active.
- Client cancellation ends the frontend wait for an active shared call. The
  gateway forwards cancellation by upstream request ID and holds the slot until
  the backend replies or the original `call_timeout_seconds` expires. A reply
  received after cancellation is discarded. A timeout fails the worker
  generation, blocks new dispatches, and lets other in-flight calls drain
  within their own deadlines before the gateway reaps the worker.
- A backend crash promptly fails all in-flight requests with a connection
  error. Session clients can reconnect. Shared mode defaults to manual gateway
  restart, with an explicit [recovery option](backend-recovery.md) for reviewed
  backends. The gateway never replays failed actions.

### Opt in to parallel calls

**Enable parallel execution only for a backend you know can safely handle
simultaneous requests on one connection.** Most stateful servers need serial
execution. Concurrent calls can abort navigation in Playwright or select
another client's page in Chrome DevTools. The gateway performs no concurrency
probing and ignores `readOnlyHint` and other tool annotations when scheduling calls.

Each request has its own upstream ID and progress token. The response and
progress are routed to the originating request, and a cancellation targets
that request's upstream ID. Explicit HTTP cancellation ends that request's SSE
stream. If a client falls behind, the bounded progress buffer can drop
notifications while preserving routing for delivered events.

If an overlapping call returns a tool or JSON-RPC error, times out, or
encounters a backend connection or protocol failure, the gateway falls back to
serial execution after in-flight calls drain. Client cancellation alone leaves
the concurrency setting in effect. Errors in calls that ran alone leave the
concurrency setting in effect. The fallback survives worker replacement and
resets on gateway restart. With parallel execution configured, health and JSON
status report `max_concurrent_calls`, effective `concurrency_per_backend` and
`degraded`.

For a managed stdio gateway, `mcp-gate share NAME --dry-run` shows the saved-catalog
advice and proposed edits. `share` and `isolate` save the chosen mode after terminal
confirmation or with `--yes`. Run the printed restart command after current actions
finish. A saved change sets `ownership_source = "operator"`, so catalog refreshes
retain the mode and still require catalog review. `isolate` removes the
shared-only keys, including `max_concurrent_calls`. Setup matches new clients by
their own proposed mode, so a recipe-less client may get a separate session gateway.
Gateway configs reject unknown fields, so `share` and `isolate` refuse to save
while the gateway's service runs a different mcp-gate binary. Update it with
setup first; `--dry-run` still shows the advice.

## Backend idle policy

`idle_seconds` is the single backend idle timeout for both ownerships. Omit it to
use the ownership default: **900 seconds for session-owned**, **off for shared**.
Use a positive integer for seconds, or **`idle_seconds = 0` to disable**. The input
`idle_seconds = "off"` is also accepted and serialized back as `0`. Omitted defaults
stay omitted when saving a config. A disabled timer has no deadline; it does not
create a zero-duration polling loop.

Only executable requests (`tools/call`, `resources/read`, `prompts/get`) extend the
timer, starting a fresh interval when they complete. Initialization, discovery
(`tools/list`, `resources/list`, `resources/templates/list`, `prompts/list`), ping,
and notifications do not. Active and admitted queued executable requests prevent
shutdown. An in-flight HTTP response also finishes before its session is closed.
Session-owned discovery previously reset expiry; it now follows this same rule.

The ownership-specific consequences are:

- **Session-owned:** expiry closes the HTTP session and its backend together;
  there is no separate HTTP idle setting. An open GET/SSE stream keeps the session
  alive for browser recordings, and closing that stream starts a fresh idle
  interval. With idle disabled, DELETE, disconnect grace after a lost GET, backend
  exit, capacity eviction and gateway shutdown still apply. A client that never
  opens GET must DELETE or explicitly close its session if it wants to release it.
- **Shared:** enabling idle is an explicit review decision that volatile backend
  state may be discarded. Shared Docker backends cannot enable it; omitted or `0`
  is valid for Docker. Open SSE streams do not extend the backend timer. Backend
  idle expiry leaves HTTP sessions open; the next executable request starts one
  fresh backend after owned-process cleanup. Idle shutdown never replays actions,
  hides an external process exit, enables crash recovery or clears a failure block.

Shared HTTP sessions have a separate 30-minute idle timeout that starts after
initialization and resets when a request, GET stream or executable call ends. An
open GET stream, in-flight request or active call keeps the session alive. The
disconnect grace period also applies after the last GET stream closes. A client
using a closed session ID receives HTTP 404 and can initialize a new session.

`session_idle_evict_seconds` keeps its name and session-only meaning. When session
idle is enabled, eviction must be no greater than `idle_seconds`. When disabled,
any eviction threshold is valid. The setting is ignored for shared ownership.

Both old names, `session_idle_seconds` and `shared_idle_seconds`, are accepted as
serde aliases. Output uses only `idle_seconds`. **Specify exactly one idle key**:
any duplicate combination (including two legacy names) is rejected, even if values
match. If a PR #12 generated config contains both old fields, retain its intended
ownership's value under `idle_seconds` and remove both old fields before loading.
An explicitly present legacy alias now sets the unified timer for either ownership;
remove a previously ignored session idle field from a shared config to retain the
shared default of off.

```toml
ownership = "session"
idle_seconds = 900 # Omit for the session default; 0 disables idle expiry.
session_idle_evict_seconds = 120
```

For a reviewed shared non-Docker backend, `ownership = "shared"` with
`idle_seconds = 900` opts into the same interval; omit the key to keep it alive.

### Set it from the CLI

```sh
mcp-gate idle NAME                     # Show persisted/effective policy.
mcp-gate idle NAME 15m --dry-run        # Preview only.
mcp-gate idle NAME 900 --yes --json     # Save 900 seconds without prompting.
mcp-gate idle NAME 1h --yes
mcp-gate idle NAME 90s --dry-run
mcp-gate idle NAME off --yes            # Saves idle_seconds = 0.
```

NAME matches a managed gateway ID, full service label, label suffix, or unambiguous
MCP server name. Seconds and integer `s`/`m`/`h` suffixes are accepted. Session
durations shorter than the configured eviction threshold are rejected; the command
never adjusts eviction implicitly. With no `--yes`, a terminal shows a preview and
asks before saving; non-interactive changes require `--yes`. `--json` is global.

The command edits only the gateway TOML under the setup/remove lock, preserves
comments and backend configuration, and replaces a legacy idle key with the new
name. It does not read token/catalog files, probe a backend, change client settings,
or restart services. The displayed value is the persisted policy; an existing
process keeps its old policy until restarted. After saving, wait for current actions
to finish and run the exact restart command printed by the CLI (`launchctl
kickstart -k ...` on macOS; PowerShell `schtasks.exe /End ...; schtasks.exe /Run ...`
on Windows). Restart discards sessions and volatile state without replay.

## Protocol boundary

| Feature | Current behavior |
| --- | --- |
| Versions | 2025-03-26, 2025-06-18, 2025-11-25; no newer stateless lifecycle |
| initialize / ping / tools/list / resources/list / resources/templates/list / prompts/list | Local SDK and reviewed catalog, no backend spawn |
| tools/call / resources/read / prompts/get | Lazy start, startup catalog check, request-scoped forwarding, serial by default |
| Cancellation / progress | Upstream IDs mapped, progress restricted to originating request |
| Client roots | Forwarded in session mode; shared defaults to rejecting roots-capable clients; explicit `ignore` keeps a fixed rootless backend context |
| Logging | Session owner only; shared unsolicited logs are not broadcast or persisted |
| Resource subscriptions / completions | Not supported; advertising backends rejected |
| Optional UI extension | Empty `io.modelcontextprotocol/ui` advertisement accepted, never negotiated/advertised downstream; core text fallback only. Other extensions and nonempty experimental capabilities rejected |
| Tool/resource/prompt-list change notifications | Re-query every catalog before next executable request; unchanged snapshot accepted, changed snapshot fails closed pending review |
| Sampling / elicitation | Not advertised to backend; SDK rejects unsupported requests |
| Remote HTTP / OAuth | Not implemented; backend transport is local stdio |

Shared mode is for reviewed servers that do not depend on client filesystem roots,
per-client identities or callbacks. It is **not** safe to mark an arbitrary browser
server shared. HTTP clients use the same configured backend credentials; this is a
single-user local gateway, not a multi-user authorization proxy. For a reviewed API
server independent of filesystem context, `shared_client_roots = "ignore"` explicitly
allows roots-capable clients without advertising or forwarding their roots upstream.
Do not enable it for a backend that needs client filesystem context. The default is
`reject`, not silently merging or choosing the first client's roots. Native Codex
0.153.4 tool calls pass with this explicit policy; live connector migration remains
separate. See [current compatibility](compatibility.md).

## Example: ordinary executable, no Node entrypoint

Adapt paths and version to the installed backend. Configuration alone connects a
supported MCP; the runtime has no branch on the server or tool name.

```toml
format_version = 2
ownership = "shared"
shared_client_roots = "reject" # Or explicitly "ignore" for a root-independent API.
listen = "127.0.0.1:8775"
token_file = "/absolute/private/backend/client-token"
catalog_file = "/absolute/private/backend/catalog.json"
state_dir = "/absolute/private/backend/state"
max_workers = 1
max_sessions = 128
max_pending_calls = 16
# Default is 1. Enable only for a reviewed, concurrency-safe shared backend.
# max_concurrent_calls = 4
queue_timeout_seconds = 30
startup_timeout_seconds = 30
call_timeout_seconds = 300
disconnect_grace_seconds = 60
# Shared ownership defaults to no idle expiry. Opt in only after review:
# idle_seconds = 900

[backend]
profile = "stdio"
command = "/absolute/path/to/mcp-server"
version = "PINNED-SERVER-INFO-VERSION"
args = ["--stdio"]
inherit_env = []

[backend.env]
PUBLIC_SETTING = "value"

[backend.env_files]
API_TOKEN = "/absolute/private/backend/api-token"
```

For an interpreted server set `command` to the interpreter and optional `entrypoint`
to the script. Optional `command_args` precede the entrypoint (for example Java's
`["--enable-native-access=ALL-UNNAMED", "-jar"]`); `args` follow it.
`directory_env = ["OUTPUT_DIR"]` allocates a private UUID directory per worker and
sets that environment variable. Optional `working_directory_env = "OUTPUT_DIR"`
also uses it as cwd. Directories remain after cleanup so downloads are recoverable;
this does not isolate deliberately selected absolute/project file destinations.
Empty optional fields are omitted from invocation serialization, preserving existing
catalog fingerprints. Secret files must be private (no group/other bits),
UTF-8; trailing CR/LF is stripped. Do not put secrets in args or plain `env` values.
The generic profile clears inherited environment except PATH, HOME, TMPDIR, LANG,
LC_ALL and explicitly listed `inherit_env` names. This is not an OS sandbox.

The Chrome profile remains `chrome_devtools`: requires `entrypoint`, `--isolated`,
session ownership, and rejects shared-browser connection arguments. It preserves
the old Chrome environment handling and disables its telemetry/update checks.

Configuration without `format_version` remains legacy v1 Chrome/session only.
Unknown fields/versions and incompatible policies are rejected. New catalogs use
format 3 and bind command/artifact hashes, arguments, profile and environment policy,
plus tools/resources/resource templates/prompts. Existing format 1/2 tools-only
catalogs remain readable; resources/prompts require regenerating format 3.

Resource descriptors and prompt definitions are cached, never resource contents or
rendered prompt results. Upstream pagination is flattened into one local snapshot.
Subscriptions and listChanged are not advertised by the immutable frontend catalog.
Unknown prompt names are rejected locally; resource URI/template resolution remains
the backend's responsibility. Tools keep `isError` execution failures; resource and
prompt failures use JSON-RPC errors. All three share the same queue, cancellation,
and recovery policy. The gateway never automatically retries them.

The UI fallback follows [MCP extension negotiation](https://modelcontextprotocol.io/extensions/overview):
an advertisement alone is not a requirement to implement the extension. The gateway
does not render MCP Apps or pretend that interactive UI support exists.
Secret-file contents are not included, permitting credential rotation. This is a
drift check, not a complete dependency-tree signature; keep package lockfiles.

## Prepare without deploying

1. Prepare a private state directory and complete the config with absolute paths.
2. `mcp-gate init-token --output /absolute/path/client-token`
3. `mcp-gate catalog --config /absolute/path/config.toml`
4. Review the generated catalog; this command runs and shuts down the real backend.
   Backend initialization behavior is upstream-specific; it may contact a service.
5. `mcp-gate check --config /absolute/path/config.toml`
6. `mcp-gate serve --config /absolute/path/config.toml`

Use a separate port/state directory for validation. `catalog` is an explicit refresh
and can overwrite that config's existing catalog; keep a reviewed copy before refresh.
`check` and discovery do not launch backend processes or read backend secret files.
`status` reports ownership, concurrency, queue limits and shared dormant/running/failed
state, in addition to sessions/workers. Runtime health exposes the effective
`idle_seconds` (seconds, or JSON null when disabled). `close-session` does not kill a shared worker.
The existing `install` command is still Chrome-specific; generic install/rollback is
a separate remaining migration gate. Never overwrite the working installation.

## Verification

`cargo xtask check` runs formatting, Clippy, size budgets and non-ignored tests:
existing Chrome-profile mock lifecycle, shared cold starts, result routing, queue
overload/timeout/cancellation, failure stickiness, isolated generic sessions, config
validation, tool-list notification handling, opt-in parallel routing and fallback,
and cleanup after hung initialization including a child process. Cached discovery is never automatically replaced with
new schemas; an actual change requires explicit regeneration and review.

```sh
DEVTOOLS_NODE=/absolute/path/to/node \
BITBUCKET_ENTRYPOINT=/absolute/path/to/bitbucket-mcp-server/build/index.js \
cargo test --locked --features test-backend --test bitbucket -- --ignored --nocapture
```

This uses the actual installed Bitbucket MCP 3.0.0 with dummy credentials, discovery
tools only and a temporary loopback API. Two real tool calls share one process and
return client-specific results. It does not verify corporate DNS, permissions, live
Bitbucket API semantics. A separate opt-in native Codex test now covers actual client
calls through this same fixture. The existing real Chrome
smoke checks its temporary profiles, cookies, traces and process cleanup separately.

Remaining boundaries: production RSS/swap measurement, desktop UI lifecycle,
sleep/wake and prolonged use, gateway SIGKILL/orphan recovery, generic
installer/rollback. No unattended restart of failed shared actions is implemented.
