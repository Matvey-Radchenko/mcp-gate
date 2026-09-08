# Security boundaries

This is a trusted-user, local-only service, not a multi-tenant browser sandbox.

- Bind is restricted to loopback. Every endpoint, including health and DELETE,
  requires a local bearer token. Token file permissions must exclude group/other.
- Requests with Origin, an unexpected Host or duplicate security-sensitive headers
  are rejected. No CORS, unauthenticated status, arbitrary target selection or shell
  command execution is exposed over HTTP.
- Configuration is trusted operator input. It selects the executable, arguments
  and environment. Keep the install prefix private. The gateway cannot protect
  against a compromised user account, altered binaries or malicious trusted config.
- HTTP session IDs prevent accidental state collisions, not hostile access between
  clients sharing the same token. Such clients can see session IDs via authenticated
  status and explicitly close those sessions.
- Browser user-data directories are temporary and per backend. No existing personal
  Chrome is attached. Upstream roots restrictions are forwarded, not bypassed with
  `--allow-unrestricted-paths`.
- Backend receives the launch environment except NODE_OPTIONS. Google usage telemetry
  and runtime update checks are disabled. Chrome's own networking and upstream CrUX
  behavior remain upstream defaults unless explicitly configured otherwise.
- Default gateway logs contain lifecycle metadata, not tool arguments/results, page
  contents, URLs or cookies. Backend stderr is discarded. Enabling SDK trace logging
  via RUST_LOG can expose protocol data: never share such logs without review.
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

No remote publishing, public issue tracker or security upload is configured.
