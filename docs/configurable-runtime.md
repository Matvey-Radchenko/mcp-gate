# Configurable runtime — unreleased

One executable, one gateway process per configured MCP, one authenticated loopback
HTTP endpoint per gateway. Clients connect directly; no per-client stdio adapters.
No live service deployment or client configuration changes are part of development.

## Ownership and request lifecycle

- `session` (default): each HTTP MCP session owns its lazy backend, including its
  roots and logging destination. Closing it does not close another session's backend.
- `shared` (explicit): the gateway owns one lazy backend across clients. Closing all
  clients does not shut it down. Shutdown of the gateway reaps it.
- Both modes execute one request (`tools/call`, `resources/read`, `prompts/get`) at a time **per backend**. `max_pending_calls`
  bounds additional admitted calls, `queue_timeout_seconds` bounds lock waiting.
  Zero pending calls means fail-fast while busy. Cancelled or expired queued calls
  do not reach the backend. These limits are not OS memory quotas.
- There is no idle eviction: long user pauses do not destroy browser state. Existing
  disconnect grace closes HTTP sessions only when no tool call is active. Clients
  that never opened GET/SSE still need DELETE or explicit administrative closure.
- An active shared call cancelled/timed out with uncertain outcome poisons the shared
  slot and closes its process. Waiting/new clients receive an error; they cannot
  silently spawn a replacement. An operator must restart that gateway. This trades
  availability for preserving serial execution and avoiding repeated side effects.
- A backend crash also fails its slot. Session mode can start fresh through a new
  HTTP session; shared mode requires gateway restart. The action is never replayed.

## Protocol boundary

| Feature | Current behavior |
| --- | --- |
| Versions | 2025-03-26, 2025-06-18, 2025-11-25; no newer stateless lifecycle |
| initialize / ping / tools/list / resources/list / resources/templates/list / prompts/list | Local SDK and reviewed catalog, no backend spawn |
| tools/call / resources/read / prompts/get | Lazy start, startup catalog check, serial request-scoped forwarding |
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
queue_timeout_seconds = 30
startup_timeout_seconds = 30
call_timeout_seconds = 300
disconnect_grace_seconds = 60

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
prompt failures use JSON-RPC errors. All three share the same queue and fail-closed
ownership policy, including timeout/cancellation; none is automatically retried.

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
state, in addition to sessions/workers. `close-session` does not kill a shared worker.
The existing `install` command is still Chrome-specific; generic install/rollback is
a separate remaining migration gate. Never overwrite the working installation.

## Verification

`cargo xtask check` runs formatting, Clippy, size budgets and non-ignored tests:
existing Chrome-profile mock lifecycle, shared cold starts, result routing, queue
overload/timeout/cancellation, failure stickiness, isolated generic sessions, config
validation, tool-list notification handling, and cleanup after hung initialization
including a child process. Cached discovery is never automatically replaced with
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
