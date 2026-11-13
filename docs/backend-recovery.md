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
  connectivity permits subsequent requests on the same process.
- Cancelling a queued request never dispatches it or stops another client's call.
- Dropping an HTTP response alone does not cancel its handler. Explicit MCP
  cancellation or session termination can cancel it. Cancellation is best-effort;
  an MCP cancellation notification has no acknowledgement proving quiescence.
- An interrupted shared call, deadline or broken backend transport retires that
  worker generation. The failed action is **never retried**. Its external outcome
  can be unknown even after the local process exits: verify writes before issuing
  a replacement or a dependent write.
- Cleanup holds the execution lock while closing the protocol and owned process
  group/Windows job. Startup abandoned before producing a Worker retains the same
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

Tests in `tests/recovery.rs` exercise timeout/cancel/crash recovery, queued-call
rejection, concurrent fresh clients, no replay, late cleanup, catalog blocks,
discovery outages, network error results, transport disconnection and cooldown.
These ordinary tests run in `cargo xtask check` and the existing native CI matrix.
Native Intel/Windows and real-backend results must be recorded separately from
local mock coverage before publication.
