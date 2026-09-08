# Repository working agreement

- Read `CONTRIBUTING.md` before changing code. Current behavior is documented in
  `docs/architecture.md`; planned work is separately scoped in `docs/compatibility.md`.
- Preserve unrelated changes. This repo is local-only: no publishing, service
  installation, credential access, or client config changes without authorization.
- One module owns one responsibility. Keep CLI wiring, HTTP/auth, protocol routing,
  session policy, process ownership and backend-specific configuration separate.
- New standard MCP servers must be connectable through configuration after the
  runtime refactor; do not add a Rust branch per server name or per tool name.
- Enforce `quality.toml`. Do not inflate budgets, minify code, move implementation
  into giant inline modules/macros, or remove useful comments to pass a check.
- Prefer functions of 40–60 logical lines; Clippy's 150-line threshold is a ceiling,
  not a target. Extract named lifecycle phases, not arbitrary numbered fragments.
- Explain ownership, cancellation, lock scope, state transitions and safety at their
  boundaries. Avoid catch-all `utils.rs` and abstractions without two real consumers.
- Never silently discard errors, retry possibly executed tool actions, log secrets
  or payloads, or destroy active state solely because no recent tool call arrived.
- New lint exceptions must be item-local with a `reason`; no crate-wide suppressions.
- Run `cargo xtask check` before handoff. For lifecycle changes also run the relevant
  explicitly authorized real-backend smoke tests. Report skipped checks accurately.
