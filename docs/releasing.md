# Release process

1. Complete every mandatory check in `release/acceptance.json` with actual evidence.
   Pending checks must not be relabelled as passed to enable the workflow.
2. Review the exact Git tree/history and npm/archive contents for private data.
   The public branch starts at a sanitized source snapshot; original local history
   and operational notes are retained separately and must not be pushed.
3. Confirm the availability/ownership of `mcp-gate`, `mcp-gate-bin-darwin-arm64`,
   `mcp-gate-bin-darwin-x64`, and `mcp-gate-bin-win32-x64`.
4. The owner authenticates separately to npm and configures trusted publishing for
   all four packages: owner `Matvey-Radchenko`, repository `mcp-gate`, workflow
   `release.yml`, environment `npm`, with publishing allowed. See the current
   [npm instructions](https://docs.npmjs.com/trusted-publishers/). No access token
   belongs in this repository or in a workflow file.
5. Make the reviewed repository public, then dispatch `Publish verified release`
   for `0.1.0`. The workflow re-runs the native matrix, publishes platform packages
   before the launcher, and creates the matching GitHub Release with checksums.
6. Verify installation from the actual registry on all three platforms, download
   each GitHub archive, verify its checksum and run its native binary on the
   corresponding clean runner. The release is not complete until these pass.

Updating npm alone changes no services. Users must run setup with the chosen
version to update or compatibly downgrade background gateways. The public tag is
`latest`; no public rollback/update/doctor command is introduced.
