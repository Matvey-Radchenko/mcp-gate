# Development and code style

Development commands do not deploy binaries, update client
settings, register launchd jobs or publish anything. Keep secrets and runtime data
outside the repository. Report vulnerabilities privately, as described in
[SECURITY.md](SECURITY.md). Do not put tokens or private configuration in an issue
or pull request.

## One check command

```sh
cargo xtask check
```

Runs source-size budgets, `rustfmt --check`, workspace-wide Clippy with warnings
treated as errors, and all non-ignored unit/mock integration tests. It includes
the `test-backend` feature. Requires Cargo, rustfmt, Clippy and local loopback access;
Cargo may need network access if locked dependencies are not cached. It never runs
the ignored real Chrome/Codex tests. Their explicit commands remain in the README.

GitHub Actions runs this same entrypoint on native macOS, Windows and Linux
runners for each release target. Before a commit or handoff, run it locally too.
There is no installed Git hook. CI also tests temporary user services, pinned real clients,
actual release update/downgrade and clean npm archive installation. See the
[native acceptance procedures](docs/native-acceptance.md) for isolated checks;
publication has a separate evidence gate.

Pull requests run only the jobs their changed files can affect.
`packaging/ci-changes.mjs` maps each path to a scope: documentation runs no
build, `npm/README.md` and `LICENSE` run the npm launcher job, packaging, npm
launcher code, lints and browser fixtures run the native matrix, and Rust
sources, tests, recipes, Cargo files and workflows also run the Docker jobs. A
path no rule names runs everything. Pushes to `main`, manual runs and the
release workflow always run everything. The `CI result` check passes only when
every job succeeded or was skipped as unaffected; use it as the required check.

Build distributable binaries with `node packaging/compile.mjs`. It remaps home and
workspace paths in compiled diagnostics before `packaging/build.mjs` stages the
npm archives. A normal local Cargo build can retain the developer's absolute paths.
CI prepares the common launcher once with `node packaging/launcher.mjs` and
`npm pack ./dist/mcp-gate --ignore-scripts --pack-destination dist`. npm names
that archive `matvei-radchenko-mcp-gate-<version>.tgz`. All eight native
jobs install that exact archive together with their native package. This avoids
platform-specific tar permission differences. `npm pack` in `npm/mcp-gate` runs a
prepack check that refuses to continue without README.md and LICENSE.
`launcher.mjs` copies both into `dist/mcp-gate` before that pack; `--ignore-scripts`
skips prepack only for the already staged tree. Clean installation checks exercise
the actual local and global commands. A final combined job verifies nine npm
tarballs and eight native archives, including identical launcher bytes across
the artifacts. `SHA256SUMS` lists the eight native archives a GitHub Release attaches.
Windows releases statically link the C runtime; CI inspects DLL imports to reject
an accidental dependency on a separate Visual C++ redistributable. See
[Rust linkage](https://doc.rust-lang.org/stable/reference/linkage.html).
CI builds and tests the static musl executables on Ubuntu 22.04 with the matching
Rust musl target and Rust's self-contained musl linking (no `musl-gcc`), then smoke-tests their npm archives only on Alpine (the packages declare
`libc: ["musl"]`, so npm skips them on glibc hosts).
The Windows ARM64 job skips the temporary Task Scheduler lifecycle tests, which
take 15-25x longer on `windows-11-arm`; Windows x64 runs them.
To select a musl build on a glibc host, set `MCP_GATE_NATIVE_TARGET` to
`linux-x64-musl` or `linux-arm64-musl` for the compile and packaging scripts.
CI also checks JavaScript syntax and runs `node --test packaging/*.test.mjs` for
the release scripts, including rejection of conflicting published archive bytes.

## Formatting and mechanical limits

- `.editorconfig`: UTF-8, LF, final newline, spaces.
- `rustfmt.toml`: stable Rust formatting, edition/style 2024, 100-column target.
  Rustfmt may retain long strings/macros: readability still needs review.
- `Cargo.toml` / `clippy.toml`: warning-free Clippy, cognitive complexity 25,
  at most 7 arguments, function-size ceiling 150 logical lines, documented unsafe.
- `quality.toml`: at most 400 physical lines per production Rust file, 800 per test
  file, 300 per development-check file. Includes comments, blanks and nested files;
  excludes dependencies/build output by scanning only declared source roots.
- New source roots must be added to `quality.toml`. Existing `tests/lifecycle.rs`
  is within the test budget; prefer splitting future suites into scenario files
  with reusable fixtures in `tests/support/` rather than growing that file.

The checker itself is a small Rust workspace member (`xtask`), not a shell script.
Inspect file budgets separately with `cargo xtask size`.

Run the opt-in [mock gateway benchmark](docs/benchmark.md) with `cargo xtask bench`.

## Readability and architecture review

One file/module should answer one question. Public boundaries describe invariants
and resource ownership. CLI parses and dispatches; HTTP authenticates and transports;
session policy decides sharing/lifetime; the worker owns and reaps subprocesses.
Backend profiles describe invocation and constraints, not transport internals.

Prefer functions of 40–60 logical lines and explicit named phases. No universal
`manager.rs`/`utils.rs`, boolean-argument soups or speculative plugin framework.
Use enums for lifecycle states and meaningful types for identities when refactoring.
Keep visibility narrow. Size checks do not prove correct module boundaries.

For fallible external inputs use `Result` with actionable, sanitized context. Explain
any invariant behind `expect`/`unwrap` in production. Tests may use them for assertions.
Do not hold blocking mutex guards across `.await`. Every child/task needs an owner
and bounded cleanup. FFI needs a nearby `SAFETY` explanation. Avoid global lint
suppression; a necessary exception belongs on the smallest item with a reason.

## Debugging

```sh
cargo build --locked --profile diagnostic --bin mcp-gate
cargo test --locked --features test-backend --test lifecycle roots_cancellation_crash_and_no_replay -- --nocapture
```

The optional diagnostic profile retains line tables and symbols; the installed
release is unchanged. Use a separate test config, state directory and loopback port
if launching it. Trace lifecycle transitions with session/request IDs, backend PID
and durations, never credentials, tool arguments/results, cookies or page contents.
Do not log the credential helper's stdout. Debug output/backtraces may include local
paths: inspect them before sharing.
