# Native acceptance procedures

Run these against generated fixtures, without migrating personal MCPs. Ordinary
checks remain `cargo xtask check`.

## Release update and compatible downgrade

```sh
node packaging/compile.mjs
node packaging/compat-build.mjs
```

The second build uses this reviewed tree with version `0.1.0-compat-fixture` in a
generated Cargo manifest and lockfile. It is an unpublished compatibility fixture,
not a historical public release. Both binaries use the production release profile
without the `test-backend` feature. The mock MCP itself is built by the ordinary
harness. The compatibility executable is never copied into `dist`.

Set `MCP_GATE_RELEASE_BINARY` to the absolute `target/release/mcp-gate` path and
`MCP_GATE_COMPAT_BINARY` to `target/compatibility/release/mcp-gate`, then run:

```sh
cargo test --locked --all-features --test installer_versions -- --ignored
```

On Windows both filenames end in `.exe`; the compatibility path is
`target/compatibility/x86_64-pc-windows-msvc/release/mcp-gate.exe`.
GitHub CI sets these paths automatically on each native runner.

The test uses a temporary home, personal Claude fixture configuration and a unique
user service. It verifies busy deferral, actual replacement in both directions,
unchanged endpoint/token/client permissions, retained artifacts, rejection of the
old session, no replay into a fresh backend, repeat-setup idempotence and removal.
This verifies format compatibility within the 0.1.0 implementation. Each later
release must also be tested against the actual prior published binary.

## Real Docker ownership

Start an existing Docker engine and review/pull the fixture image explicitly before
testing. Set `DOCKER_BINARY` to its absolute CLI path and `DOCKER_IMAGE` to an
already-pulled Linux Node image by `name@sha256:...`, then run:

```sh
cargo test --locked --all-features --test docker -- --ignored
```

The fixture uses `--pull=never`, a unique label and its returned container IDs.
It verifies lazy starts, independent connection state, removal of one owned
container while another connection and an independent container remain running,
and no automatic restart after a crash. Cleanup retains the original Docker cwd
and environment. Its Linux container does not establish Linux host support.

Local macOS ARM64 evidence on 2026-09-09 used Docker Engine 28.3.2 and Node 24.20.0
from `node@sha256:e67514e5d0f6c46656005e1b693b2ec9d52e80b641307de684d4a015ba7a4eaf`.
The test passed; Windows and macOS Intel Docker acceptance remains outstanding.

The native CI matrix also calls `docker-intel.yml`, using a dedicated Colima
profile on `macos-15-intel`, the same reviewed Node image and the same Docker test.
The profile explicitly shares the temporary fixture directory and is removed after
the check. Versions are printed in the run log; this is test infrastructure, not
software installed by mcp-gate. Colima's own [native macOS integration](https://github.com/abiosoft/colima/blob/main/.github/workflows/macos-integration.yml)
uses this runner. A configured workflow is not passing evidence: Intel remains
pending until this job completes successfully.

`docker-windows.yml` exercises the native Windows gateway and `docker.exe` with a
dedicated WSL2 Linux engine. Its Ubuntu 24.04.4 image is checksum-pinned to the
[Microsoft WSL registry](https://github.com/microsoft/WSL/blob/master/distributions/DistributionInfo.json).
Only a new, recorded CI distribution is created and removed. The engine listens
on WSL loopback and is reached through [localhost forwarding](https://learn.microsoft.com/en-us/windows/wsl/networking).
The Node fixture is passed as literal `node -e` arguments in this test: standalone
Docker CLI with WSL does not provide Docker Desktop's Windows drive translation.
This checks native process/argument/container ownership, not Desktop installation
or bind-mount translation. It remains pending until that native job passes.

## Actual login and protected-folder acceptance

This gate requires an actual new OS login on each supported platform. A workflow
calling `launchctl bootstrap` or `schtasks /Run` is not that evidence. Use a clean
test account or disposable native machine and the exact inspected release archive.

1. Configure a personal, project-local Claude fixture MCP using an absolute path
   to the generated `mock-backend` executable. Keep the common `.mcp.json` empty
   and record its bytes. Use only the dedicated fixture project and account.
2. Run the release binary's `setup --client claude-code --server fixture --project
   PROJECT --yes --json`. Record the gateway ID, permanent binary path and hash,
   config/catalog hashes and service registration. Do not include token contents.
3. Confirm passive `status` reports zero workers, connect the test client and call
   its state tool once. Leave that MCP connection open, then sign out and sign in
   again (or reboot). Record the actual login event and OS/client versions.
4. Before running setup or manually starting the service, run passive `status`.
   Require a reachable gateway with the same permanent binary, zero sessions and
   zero workers. Confirm config/catalog and common-project bytes are unchanged.
5. Connect afresh. The first tool call must start a new backend with empty state;
   previous actions must not appear. Remove only the fixture and verify the direct
   configuration and retained artifact files.

For macOS, also repeat the fixture with its backend under the test account's
protected Documents directory. Record behavior before and after the account owner
grants the required OS access. Client tool approvals and macOS privacy grants are
separate; the installer must not change TCC or move the backend to bypass it.

Retain sanitized results with the tested binary hashes. Until these checks have
actually been completed, their release evidence stays pending.
