# Release process

1. Complete every mandatory check in `release/acceptance.json` with actual evidence.
   Pending checks must not be relabelled as passed to enable the workflow.
2. Review the exact Git tree/history and npm/archive contents for private data.
   The public branch starts at a sanitized source snapshot; original local history
   and operational notes are retained separately and must not be pushed.
3. Confirm the availability/ownership of `mcp-gate`, `mcp-gate-bin-darwin-arm64`,
   `mcp-gate-bin-darwin-x64`, and `mcp-gate-bin-win32-x64`.
4. Make the reviewed repository public. The owner authenticates separately to npm
   with 2FA. npm requires a package to exist before adding its first trusted
   publisher; neither OIDC nor staging can bootstrap a brand-new name. See
   [npm trust prerequisites](https://docs.npmjs.com/cli/v11/commands/npm-trust/)
   and [staging prerequisites](https://docs.npmjs.com/staged-publishing/).
5. For the first release, configure the `npm` GitHub environment to hold the publish
   job for owner approval. Dispatch `Publish verified release` for `0.1.0`. After
   native checks finish, download that run's `native-*` artifacts and generate
   `SHA256SUMS` with `node packaging/checksums.mjs artifacts`. The owner publishes
   those exact three platform `.tgz` files first and the launcher `.tgz` last,
   using `npm publish ARCHIVE --access public --tag latest`. Do not publish a dummy
   version or rebuild the artifacts between this step and resuming the workflow.
   Configure trusted publishing for all four now-existing packages: GitHub owner
   `Matvey-Radchenko`, repository `mcp-gate`, workflow `release.yml`, environment
   `npm`, with direct publishing allowed. Follow the current
   [npm instructions](https://docs.npmjs.com/trusted-publishers/). Then resume the
   waiting publish job. It verifies the existing packages' exact SHA-512 integrity
   and `latest` tags before creating the GitHub Release. Later versions publish
   through OIDC, platform packages first. No access token belongs in the repository
   or workflow. A conflicting immutable version aborts the workflow.
6. Verify installation from the actual registry on all three platforms, download
   each GitHub archive, verify its checksum and run its native binary on the
   corresponding clean runner. The release is not complete until these pass.

Updating npm alone changes no services. Users must run setup with the chosen
version to update or compatibly downgrade background gateways. The public tag is
`latest`; no public rollback/update/doctor command is introduced.
