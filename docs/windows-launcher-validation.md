# Windows npm launcher: long executable paths

Local validation used Windows x64 and Node.js 24.21.0 against v0.1.1, based on
`3bc397ef090ba70f26ac638abd62b328cded1d7e`.

The published native mcp-gate 0.1.1 executable was resolved under a pnpm dlx cache
to an absolute path of 263 characters. Launching that existing file with Node's
`spawnSync(path, ['--version'])` failed with `ENOENT`. Passing the same path through
`node:path.toNamespacedPath` succeeded with exit code 0 and `mcp-gate 0.1.1` on
stdout. No executable bytes or filesystem permissions were changed between those
two checks. The specific cache path and user profile are omitted.

The launcher now supplies that extended-length path to `spawn`. On POSIX,
`toNamespacedPath` leaves the path unchanged. It does not introduce a shell,
copy the binary or change the existing argument, stdio or signal forwarding.

## Regression test

```sh
node --test packaging/windows-launcher.test.mjs
node --test packaging/*.test.mjs
```

The new Windows tests run the actual launcher with a temporary optional native
package. A copied Node executable serves as the child executable: the assertions
check real process creation, forwarded Unicode/space/quote arguments, stdout and
exit code 7. Both short and longer-than-260-character executable paths are covered.
There is no mocked spawn or extra production export.

Before the launcher fix, the short-path case passed and the long-path case failed
with the launcher's native-start error. After the fix both passed. The complete
packaging suite passed 14 tests with one unsupported-platform test skipped on
Windows. The Windows regression tests are automatically included by the existing
native CI command and skip on other platforms.

UNC paths, signal behavior and older supported Node releases were not exercised
locally. This report does not claim native Windows logout/login acceptance or
results for macOS.
