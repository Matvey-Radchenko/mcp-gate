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

## Sample results: Linux x86_64 VM (2026-11-22)

These numbers come from one machine and one run. They are not a reference
result. The command was `cargo xtask bench` (200 calls per client, 1, 3 and 5
clients, release build) on 2026-11-22 at 18:01 CET. The VM was not idle: its load
average was about 4.7 on 8 vCPUs.

| Machine detail | Value |
| --- | --- |
| Host | Cloud VM `grok-bot-vm` (not a developer laptop) |
| `uname -a` | `Linux 6.12.113+ #1 SMP PREEMPT_DYNAMIC x86_64 GNU/Linux` |
| CPU model (`/proc/cpuinfo`) | Intel(R) Xeon(R) Processor |
| CPU count | 8 virtual CPUs |
| RAM (`/proc/meminfo`) | 16,386,416 KiB (15.6 GiB) |
| `rustc -V` | `rustc 1.100.0 (0ef1bc386 2026-11-09)` |

| Mode | Clients | p50 (ms) | p95 (ms) | Cold init (ms) | Cold call (ms) | Idle RSS (KiB) | Idle private (KiB) | Loaded RSS (KiB) | Loaded private (KiB) |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| direct | 1 | 0.051 | 0.154 | 17.349 | 19.867 | 5064 | 2844 | 5308 | 3024 |
| shared | 1 | 44.004 | 48.017 | 25.950 | 114.215 | 12628 | 10360 | 13352 | 11084 |
| session | 1 | 44.009 | 47.953 | 10.467 | 95.179 | 12804 | 10580 | 13540 | 11316 |
| direct | 3 | 0.048 | 0.124 | 1.855 | 2.873 | 15376 | 1444 | 16016 | 1828 |
| shared | 3 | 44.072 | 48.238 | 10.345 | 101.536 | 12608 | 10364 | 13652 | 11408 |
| session | 3 | 44.003 | 44.363 | 10.804 | 97.492 | 12804 | 10644 | 13828 | 11668 |
| direct | 5 | 0.061 | 0.153 | 1.640 | 3.445 | 25420 | 2364 | 26500 | 3060 |
| shared | 5 | 44.008 | 47.811 | 10.947 | 97.455 | 12960 | 10748 | 14252 | 12040 |
| session | 5 | 44.007 | 44.279 | 10.832 | 97.186 | 13024 | 10740 | 14532 | 12248 |

How to read these numbers:

- **The ~44 ms gateway p50 is a TCP artifact, not routing cost.** The gateway's
  listener does not set `TCP_NODELAY`. A response written in more than one segment
  then waits for the client's delayed ACK, which takes about 40 ms on Linux
  loopback. As a local experiment (not part of this branch), the same machine
  enabled `TCP_NODELAY` on accepted sockets. Shared mode then measured p50/p95 of
  0.155/0.287 ms with 1 client and 0.213/0.424 ms with 3, and the cold first call
  dropped to about 14 ms.
- Direct rows sum one mock process per client. Gateway rows count only the
  gateway, not its mock backends. In session mode, each client's backend adds
  roughly one direct-row process on top.
- Direct mode with 1 client also covers the first exec of the freshly built
  mock, so its cold-start columns are higher than the 3- and 5-client rows.
