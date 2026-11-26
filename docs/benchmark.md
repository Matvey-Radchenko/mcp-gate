# MCP transport benchmark

`cargo xtask bench` compares direct MCP over stdio with mcp-gate over loopback
Streamable HTTP. Direct stdio launches one mock backend per client. Shared mode
uses one gateway and one backend. Session mode uses one gateway and a separate
backend for each client. Both gateway modes start their backends on demand.

## Run it

On Linux or macOS, run from the repository root with the locked Cargo dependencies
already cached. On an uncached checkout, `cargo fetch --locked` prepares the cache.
The benchmark builds the driver, gateway, and existing `test-backend` fixture in
release mode. The nested builds run offline. Set `CARGO_NET_OFFLINE=true` to require
the initial xtask build to use the cache too.

```sh
cargo xtask bench                                      # 200 calls per session, 1, 3 and 5 clients
cargo xtask bench --calls 500 --concurrency 1,2,4 --json
cargo xtask bench --mode direct                         # direct stdio only
cargo xtask bench --help
```

Each scenario uses a temporary working directory. Gateway runs use an ephemeral
loopback port, a generated bearer token, and temporary config, catalog, and state
paths. The driver runs `mcp-gate catalog --config` and `mcp-gate serve --config` as
the integration tests do. The mock backend runs over stdio and makes no network
requests. The benchmark is opt-in and runs separately from `cargo xtask check`.

## Method

- **Workload.** Each client initializes, sends `notifications/initialized`, and
  lists tools. Once all clients finish discovery, they begin issuing calls
  concurrently with one in-flight request per client. Every request sets the
  same value with the mock's cheap `state` tool and validates the returned value.
  Backend idle expiry is disabled and the shared queue accommodates the clients.
- **Latency.** p50 and p95 are nearest-rank percentiles over all tool calls in a
  row, with one sample per call. Timing includes request encoding, transport,
  any queue wait, and decoding and validating the complete response. The first
  call in each session is included, so short runs can reflect lazy startup in
  the percentiles. HTTP connections can be reused. Clients send POST requests
  and close with DELETE and use no long-lived GET streams.
- **Memory.** Idle is sampled after all clients discover tools and before the
  first call, when the gateway still has zero backends. Loaded is sampled after
  exactly the configured calls per client finish and before clients disconnect.
  Linux RSS is `VmRSS` from `/proc/<pid>/status` and private memory is
  `Private_Clean + Private_Dirty` from `/proc/<pid>/smaps_rollup`. macOS uses
  `ps -o rss` and reports private memory as unavailable. Direct rows sum memory
  across the mock backend processes and RSS can count shared pages more than once.
  File-backed pages counted as private with one process can be shared with
  several processes. Shared and session rows report only the gateway process.
- **Cold start.** A separate pass starts fresh processes. The clock starts just
  before spawning the gateway or, in direct mode, before spawning the batch of
  mock processes. It records the first successful MCP initialize response and
  the first successful tool call across the cohort, in separate columns.
  First-call time includes waiting for all clients to finish discovery and, in
  gateway mode, lazy backend startup and catalog verification. Gateway startup
  includes authenticated readiness polling at 1 ms sleep intervals. Binary
  builds, catalog generation, and memory sampling are outside the cold timer.

These are point-in-time memory samples from a single run with fresh processes and
the operating system's existing file caches. Process scheduling, cache state, and
RSS accounting can vary between runs. The mock measures overhead for a tiny local
request. Real backends can have different startup, memory, payload, and contention
profiles.

## Sample results: Linux x86_64 VM (2026-11-26)

These numbers come from one machine and one run of mcp-gate 0.2.0
(`2dce201`). They are not a reference result. The command was `cargo xtask bench`
(200 calls per client, 1, 3 and 5 clients, release build) on 2026-11-26 at
11:52 CET. The VM was mostly idle: its load average was 0.07 before the run and
about 1 after it, on 8 vCPUs.

| Machine detail | Value |
| --- | --- |
| Host | Cloud VM (not a developer laptop) |
| `uname -srm` | `Linux 6.12.113+ x86_64` |
| CPU model (`/proc/cpuinfo`) | Intel(R) Xeon(R) Processor |
| CPU count | 8 virtual CPUs |
| RAM (`/proc/meminfo`) | 16,386,416 KiB (15.6 GiB) |
| `rustc -V` | `rustc 1.100.0 (0ef1bc386 2026-11-09)` |

| Mode | Clients | p50 (ms) | p95 (ms) | Cold init (ms) | Cold call (ms) | Idle RSS (KiB) | Idle private (KiB) | Loaded RSS (KiB) | Loaded private (KiB) |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| direct | 1 | 0.038 | 0.044 | 2.232 | 3.348 | 5140 | 3040 | 5288 | 3124 |
| shared | 1 | 0.143 | 0.187 | 11.256 | 16.359 | 13296 | 11004 | 14100 | 11808 |
| session | 1 | 0.133 | 0.312 | 10.179 | 14.831 | 13444 | 11192 | 13932 | 11680 |
| direct | 3 | 0.049 | 0.077 | 2.246 | 3.009 | 15760 | 1452 | 16144 | 1772 |
| shared | 3 | 0.196 | 0.268 | 10.566 | 15.383 | 13240 | 11096 | 14276 | 12132 |
| session | 3 | 0.196 | 0.357 | 17.298 | 21.501 | 13356 | 11100 | 14460 | 12200 |
| direct | 5 | 0.064 | 0.110 | 1.801 | 3.268 | 25748 | 2348 | 26512 | 2920 |
| shared | 5 | 0.313 | 0.498 | 10.118 | 14.444 | 13764 | 11540 | 14980 | 12756 |
| session | 5 | 0.238 | 0.425 | 10.639 | 13.720 | 13548 | 11276 | 15036 | 12764 |

How to read these numbers:

- The gateway adds roughly 0.1 to 0.25 ms per call at p50 over direct stdio for
  this mock, which answers instantly. Real servers usually take far longer per call.
- Direct rows sum one mock process per client, so direct memory grows by about
  5 MB per client here. Gateway rows count only the gateway, which stays at about
  13 to 15 MB from one to five clients. Shared mode adds one mock backend on top;
  session mode adds one per client, roughly one direct-row process each.
- The mock backend is tiny. With a real server such as a 400 MB JVM process, the
  saving comes from shared mode running one backend instead of one per session,
  and from lazy startup when a session never calls the server.
- Cold call includes lazy backend startup and catalog verification. Direct mode
  has no catalog or HTTP readiness step.
- An earlier run on 2026-11-22, before the gateway set `TCP_NODELAY` on accepted
  connections, measured a p50 of about 44 ms for both gateway modes. That was a
  delayed-ACK artifact on Linux loopback, fixed in 0.2.0.

## Real server example: Context7 (2026-11-26)

The README figures come from this separate measurement, not from
`cargo xtask bench`. It used the same Linux x86_64 VM, Node.js 24.22.1, npm 11.21.1,
and `@upstash/context7-mcp@4.7.3` started with `npx -y`, without an API key.
The numbers are for 4.7.3, the version that was current when the measurement
started. Context7 4.8.0, released on 2026-11-24, also works with mcp-gate 0.2.0: it
declares the `io.modelcontextprotocol/skills` extension, which the gateway accepts
and does not forward to clients.
Each session sent `initialize`, `notifications/initialized`, `tools/list`, and one
`resolve-library-id` call. Memory was sampled 3 seconds after the last call.

Memory is PSS (proportional set size) from `/proc/<pid>/smaps_rollup`, summed over
each process tree: the `npm exec` wrapper, its `sh`, and the Context7 Node.js
process, plus the gateway in the mcp-gate row. PSS splits pages shared between
processes, so the sum does not double-count them. Summed RSS is shown for comparison.

| Setup | Processes | PSS total | RSS total |
| --- | --- | ---: | ---: |
| One session, direct stdio | 1 Context7 tree | 169,125 KiB (≈ 165 MB) | 226,984 KiB |
| Five sessions, direct stdio | 5 Context7 trees | 623,189 KiB (≈ 610 MB) | 1,129,540 KiB |
| Five sessions through one shared mcp-gate gateway | gateway + 1 Context7 tree | 189,524 KiB (≈ 185 MB) | 249,584 KiB |

In the last row the gateway process alone had an RSS of 14,068 KiB and a PSS
of 11,793 KiB. The five clients connected over loopback Streamable HTTP to a
gateway with `ownership = "shared"`, configured as in
[configurable runtime](configurable-runtime.md#example-ordinary-executable-no-node-entrypoint).

This is one run on one machine. Node.js memory varies with version, heap growth,
and workload, and macOS and Windows account memory differently.
