# Development and code style

This is a local project. Development commands do not deploy binaries, update client
settings, register launchd jobs or publish anything. Keep secrets and runtime data
outside the repository.

## One check command

```sh
cargo xtask check
```

Runs source-size budgets, `rustfmt --check`, workspace-wide Clippy with warnings
treated as errors, and all non-ignored unit/mock integration tests. It includes
the `test-backend` feature. Requires Cargo, rustfmt, Clippy and local loopback access;
Cargo may need network access if locked dependencies are not cached. It never runs
the ignored real Chrome/Codex tests. Their explicit commands remain in the README.

The check is local and opt-in, not an installed Git hook or hosted CI. Before a
commit or handoff, run it; future CI should use this same entrypoint.

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
