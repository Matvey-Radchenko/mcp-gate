# Security boundaries

This is a trusted-user, local-only service, not a multi-tenant browser sandbox.

- Bind is restricted to loopback. Every endpoint, including health and DELETE,
  requires a local bearer token. Private files exclude group/other access on macOS;
  Windows uses a protected ACL granting access only to the current user and SYSTEM.
- Requests with Origin, an unexpected Host or duplicate security-sensitive headers
  are rejected. No CORS, unauthenticated status, arbitrary target selection or shell
  command execution is exposed over HTTP.
- Configuration is trusted operator input. It selects the executable, arguments
  and environment. Keep the install prefix private. The gateway cannot protect
  against a compromised user account, altered binaries or malicious trusted config.
- HTTP session IDs prevent accidental state collisions, not hostile access between
  clients sharing the same token. Such clients can see session IDs via authenticated
  status and explicitly close those sessions.
- Reviewed browser recipes and the legacy Chrome profile use independent browsers
  with temporary per-backend user-data directories; they do not attach personal
  Chrome. Other configured commands retain their own browser/profile behavior:
  `session` isolates the MCP connection, not arbitrary external resources. Upstream
  roots restrictions are forwarded, not bypassed with `--allow-unrestricted-paths`.
- Managed stdio backends receive the recorded launch context, a small base process
  environment, and explicitly configured values or private file references. The
  legacy Chrome profile inherits its launch environment except NODE_OPTIONS and
  disables that MCP's usage telemetry/update checks. Other MCP networking and
  telemetry remain controlled by their original command and environment.
- Default gateway logs contain lifecycle metadata, not tool arguments/results, page
  contents, URLs or cookies. Backend stderr is discarded; the CLI uses a fixed
  logging filter instead of ambient RUST_LOG. Private recovery journals contain
  original configuration values and must be treated as credential-bearing files.
- The `headers` command intentionally emits a credential for the local client. Do not
  run it in recorded output or commit its result. The generated Codex example contains
  only a command/path and URL, not the credential.
- Capacity limits bound session and backend counts; they do not impose a hard Chrome
  memory limit. A resource-heavy page can still consume significant RAM.
- A timeout/cancellation does not prove that a browser action did not occur. No
  automatic replay is performed. Callers must inspect state before retrying writes.
- SIGTERM/DELETE cleanup is covered by tests. SIGKILL and OS crash cleanup are not
  guaranteed by this release. This boundary is explicit rather than hidden by an
  unsafe wildcard kill of all Chrome or Node processes.

Diagnostics are not uploaded automatically. Release publication requires completed
acceptance evidence and owner-configured npm authorization. Never include tokens,
private configuration files or backend payloads in issue reports.
