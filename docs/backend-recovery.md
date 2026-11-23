# Backend recovery

Shared recovery is explicitly opt-in in the gateway's private TOML:

```toml
format_version = 2
ownership = "shared"
max_workers = 1
shared_recovery = "restart"
```

Omitting `shared_recovery` retains `manual` behavior. Setup does not enable this
policy automatically; existing recipes have not gained an unverified restart
guarantee. Enable it only after reviewing that backend's volatile state, fixed
identity, exclusive resources and remote-write semantics. Session backends and
Docker are excluded. A running older daemon must be explicitly updated; replacing
the CLI does not change it. Older strict readers reject the new configuration
field rather than interpreting it with different behavior.

## Request and process boundaries

- An ordinary tool `isError` result or MCP error is returned without stopping the
  backend. This includes network/VPN failures reported by the backend. Restoring
  connectivity permits subsequent requests on the same process. With
  `max_concurrent_calls` above one, an error on an overlapping request switches
  the slot to serial execution, including after worker replacement.
- Cancelling a queued request never dispatches it or stops another client's call.
- Explicit MCP cancellation, a disconnected HTTP POST response, or session
  termination ends the originating client's wait for a shared call. For an
  already dispatched call, the gateway sends a best-effort
  `notifications/cancelled` with the upstream request ID. Dropping a standalone
  GET stream does not cancel outstanding POSTs. Session-owned backends keep
  their session lifetime rules.
- A cancelled shared call holds its execution permit and admission slot until
  the backend replies or the original call deadline expires. The gateway
  discards the late reply. The cancellation notification is not an
  acknowledgement that execution stopped, and client cancellation alone does
  not change the worker generation or trigger recovery.
- A missed call deadline (even after client cancellation), a broken backend
  connection, or a backend protocol failure retires the worker generation. The
  failed action is **never retried**. Its external outcome can be unknown even
  after the local process exits. Verify writes before issuing a replacement or
  a dependent write.
- Cleanup waits for already dispatched calls and holds all execution permits
  while closing the protocol and owned process group/Windows job. Startup
  abandoned before producing a Worker retains the same
  single-worker capacity until its ProcessOwner finishes cleanup. No new worker
  starts while old cleanup is pending. Detached/unowned resources are not claimed.
- Requests queued before a generation change fail as **not dispatched**. A fresh
  request may start one replacement backend after cleanup and cooldown. The
  replacement must pass the pinned catalog check before executing anything.
- Recovery discards volatile backend state; it does not restore sessions or actions.
  Browser/TDLib state must not be assumed recoverable merely because it is shared.

## Backoff and diagnostics

Failures introduce 1s and 2s cooldowns. Three failures within 60s prevent another
start until the oldest failure leaves that window. There is no autonomous restart
loop; starts remain lazy and require a new request. Returning from a VPN outage
therefore does not cause actions to be replayed. No generic error proves the VPN
was the cause, and the gateway does not change VPN/network settings.

Discovery failure/timeout is transient under the restart policy. An actually
different catalog is blocked until reviewed; it is never accepted by recovery.
Manual policy remains blocked after uncertain execution. `status` distinguishes
blocked/recovering/dormant/running, preserves legacy runtime state, and reports a
typed failure reason and cooldown. An older health response without backend state
is reported as unknown, not ready merely because its worker count is zero.

`state_dir/recovery.jsonl` records typed failure and cleanup events with time,
daemon PID and generation, never tool arguments/results or peer error text. It is
private and rotates at 64 KiB to one previous file. Journal failure is reported to
the service log. Previous history is diagnostic only; it never restores actions.

Tests in `tests/recovery.rs` exercise cancellation and late replies, deadline and
crash recovery, queued-call rejection, concurrent fresh clients, no replay, late
cleanup, catalog blocks, discovery outages, network error results, HTTP
disconnection and cooldown.
These ordinary tests run in `cargo xtask check` and the existing native CI matrix.
Native Intel/Windows and real-backend results must be recorded separately from
local mock coverage before publication.
