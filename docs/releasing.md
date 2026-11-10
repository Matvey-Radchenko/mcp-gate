# Release process

1. Complete every mandatory check in `release/acceptance.json` with actual evidence.
   Pending checks must not be relabelled as passed to enable the workflow.
2. Review the exact Git tree/history and npm/archive contents for private data.
   The public branch starts at a sanitized source snapshot; original local history
   and operational notes are retained separately and must not be pushed.
3. Confirm the availability/ownership of `mcp-gate`, `mcp-gate-bin-darwin-arm64`,
   `mcp-gate-bin-darwin-x64`, and `mcp-gate-bin-win32-x64`.
4. Confirm the reviewed repository is public. The owner authenticates to npm
   separately, with account 2FA. Trusted publishing needs npm CLI 11.5.1 or newer
   and Node.js 22.14 or newer; `npm trust` needs npm CLI 11.15.0 or newer. The
   publish job uses Node.js 24. A trusted publisher can be added only after the
   package exists
   ([npm trust prerequisites](https://docs.npmjs.com/cli/v11/commands/npm-trust/)).
   `npm stage publish` can create a new name, but it also publishes a public
   `0.0.0-stage` placeholder
   ([staged publishing](https://docs.npmjs.com/staged-publishing/)). Do not stage
   or publish a placeholder for this release.
5. Keep the `npm` GitHub environment as an approval hold. Dispatch
   `Publish verified release` from `main` for `0.1.0`. After the native jobs
   finish, download that run's `native-*` artifacts and run
   `node packaging/checksums.mjs artifacts`. `SHA256SUMS` lists the three native
   `.tar.gz` archives that `gh release create` attaches. The same command still
   requires all four npm `.tgz` files and rejects a launcher tarball whose bytes
   differ across runners. Those npm archives are not Release assets.
   The owner then publishes the three platform `.tgz` files first and the launcher
   `.tgz` last, from an interactive 2FA session:

   ```sh
   npm publish ARCHIVE --access public --tag latest
   ```

   Do not pass `--provenance`. Provenance is created for a public package from a
   public repository when trusted publishing runs on GitHub Actions or GitLab CI
   ([trusted publishers](https://docs.npmjs.com/trusted-publishers/),
   [provenance](https://docs.npmjs.com/generating-provenance-statements/)).
   Do not rebuild the artifacts before resuming the held job. The job checks the
   registry SHA-512 integrity and `latest` tag of those exact bytes, then creates
   the GitHub Release. Because the versions already exist, it does not call
   `npm publish` and therefore does not count as a trusted publisher's first
   successful publish.
6. Create the trusted publisher immediately before the first workflow run that
   publishes a version the registry does not already have. On npmjs.com, for each
   of the four packages, add GitHub Actions with user `Matvey-Radchenko`,
   repository `mcp-gate`, workflow filename `release.yml`, and environment `npm`.
   A publisher created after 3 September 2026 allows `npm stage publish` only
   until the owner also allows direct `npm publish`. Allow direct `npm publish`.
   `packaging/publish.mjs` runs `npm publish`, including `--tag latest` and
   `--provenance`. Trusted publishing generates provenance without that flag; the
   provenance guide still shows it, and this workflow keeps it. Do not grant
   `npm dist-tag`: the workflow does not call `npm dist-tag`.
   From npm CLI 11.15.0, neither permission defaults on. The equivalent is:

   ```sh
   npm trust github PACKAGE --file release.yml --repo Matvey-Radchenko/mcp-gate --env npm --allow-publish
   ```

   The website allows up to 10 publishers per package. The `npm trust` docs
   currently say the registry stores one configuration per package; one publisher
   for `release.yml` satisfies that stricter statement.
   A new publisher that does not complete a successful publish within 2 days
   expires and cannot be edited. Delete it, create another one, and publish
   inside the new 2 days
   ([configuration expiry](https://docs.npmjs.com/trusted-publishers/)).
   Create it when the next release workflow is ready to publish, not while the
   0.1.0 approval is waiting and not days ahead. No npm token belongs in the
   repository or workflow. A conflicting immutable version aborts the workflow.
   Later versions publish through OIDC, platform packages first.
7. Verify installation from the actual registry on all three platforms. Download
   each GitHub archive, verify its checksum, and run its native binary on the
   matching clean runner. The release is not complete until these pass.
8. Homebrew is optional. `Publish verified release` renders
   `packaging/homebrew/mcp-gate.rb.tmpl` from the Release `SHA256SUMS` and pushes
   `Formula/mcp-gate.rb` to the already created `Matvey-Radchenko/homebrew-tap`
   when the `HOMEBREW_TAP_TOKEN` secret is present. The token needs contents
   write on that tap. This workflow does not create the tap. Without the secret,
   the job logs a skip and does not clone or push.
   The formula installs the macOS Release archives (`on_arm` / `on_intel` inside
   `on_macos`, plus `depends_on :macos`). It has no `service do` block: mcp-gate
   manages its own LaunchAgents. Upgrades still require `mcp-gate setup`.

Updating npm or Homebrew alone changes no services. Users must run setup with the
chosen version to update or compatibly downgrade background gateways. The public
tag is `latest`; no public rollback/update/doctor command is introduced.

On 2026-09-09 the repository's `npm` environment was configured to require the
owner's approval and permit deployments only from `main`. This prepares the hold
needed for the first package bootstrap. It does not authenticate to npm, create
trusted publishers, publish a package or mark the acceptance gates as passed.
