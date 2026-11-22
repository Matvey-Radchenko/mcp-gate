# Release process

1. Complete every mandatory check in `release/acceptance.json` with actual evidence.
   Pending checks must not be relabelled as passed to enable the workflow.
   `passed-with-limitations` is allowed only together with evidence and a non-empty
   `remaining` or `limitations` list. 0.1.0 uses that status for native login and
   macOS protected-folder access: native acceptance is macOS ARM64 only, the
   Rosetta 2 Intel reboot is postponed, Windows was smoke-tested under Wine, and
   the TCC first-prompt/deny path was not observed. Do not delete those limitations
   to make the workflow pass. Wine's missing protected DACL is a Wine 7.7 limit,
   not a Windows product bug.
   Record a completed CI run for the expanded eight-target matrix in
   `native-matrix` before dispatch. The existing acceptance entries describe
   historical runs. Evidence for Windows ARM64 and the two static musl builds
   is pending. The release workflow also runs the full matrix before publication.
2. Review the exact Git tree/history and npm/archive contents for private data.
   The public branch starts at a sanitized source snapshot; original local history
   and operational notes are retained separately and must not be pushed.
3. The next release with new targets needs a one-time owner publish for each
   of these platform packages before npm can configure their trusted publishers.
   Use a new stable release version and align Cargo, npm and acceptance metadata.

   | New npm package | Artifact with the tarball to bootstrap |
   | --- | --- |
   | `@matvei-radchenko/mcp-gate-bin-win32-arm64` | `native-win32-arm64` |
   | `@matvei-radchenko/mcp-gate-bin-linux-x64-musl` | `native-linux-x64-musl` |
   | `@matvei-radchenko/mcp-gate-bin-linux-arm64-musl` | `native-linux-arm64-musl` |

   Follow the same owner-publish procedure used for the glibc Linux packages
   in 0.1.2. The launcher command stays `mcp-gate`. Each package uses public
   access. Use the final version's tarballs for the bootstrap.
4. Confirm the reviewed repository is public. Trusted publishing needs npm CLI
   11.5.1 or newer and Node.js 22.14 or newer. `npm trust` needs npm CLI 11.15.0
   or newer and account-level 2FA
   ([npm trust](https://docs.npmjs.com/cli/v11/commands/npm-trust/),
   [trusted publishers](https://docs.npmjs.com/trusted-publishers/)).
   The publish job uses Node.js 24 and installs `npm@11.15.0`. A trusted
   publisher requires an existing package. Use direct `npm publish` in this
   workflow.
5. Keep the `npm` GitHub environment as the approval hold. Dispatch
   `Publish verified release` from `main` after verifying the trusted
   publishers for the existing packages. Bootstrap the three new packages
   during that run, as described below. The publish job has `id-token: write`
   permission and uses the `npm` environment. It does not set `registry-url` on `setup-node` and
   it unsets `NODE_AUTH_TOKEN` and `NPM_TOKEN` before publishing. An Actions
   `NODE_AUTH_TOKEN` would make npm skip OIDC.
   `packaging/publish.mjs` publishes an absolute tarball path, platform packages
   first and the launcher last:

   ```sh
   npm publish <absolute-tarball> --provenance --access public --tag latest
   ```

   If that exact version is already on the registry, the job checks its SHA-512
   integrity and `latest` tag instead of publishing again. It then creates the
   GitHub Release from the same artifacts. Do not rebuild them while the
   environment approval is waiting.

   After this run's native jobs finish and while the `npm` environment approval
   is waiting, download the three artifacts listed above from that same run.
   From the owner's Mac, with npm CLI 11.15.0 or newer and account-level 2FA,
   publish only the three new platform tarballs:

   ```sh
   npm publish <absolute-tarball> --access public --tag latest
   ```

   The bootstrap uses the owner's npm login, so those versions have no CI
   provenance attestation. The workflow verifies their SHA-512 integrity and
   `latest` tags before publishing the launcher. Later versions of the platform
   packages can use OIDC with provenance.
6. Configure one trusted publisher per package. Check the existing publishers
   before dispatch and configure the new ones after bootstrapping their packages.
   A new publisher that does not complete a successful publish within 2 days
   expires. On npmjs.com, open each package and add a GitHub Actions publisher
   with these fields:

   | Field | Value |
   | --- | --- |
   | Organization or user | `Matvey-Radchenko` |
   | Repository | `mcp-gate` |
   | Workflow filename | `release.yml` (the filename only, not `.github/workflows/release.yml`) |
   | Environment name | `npm` |
   | Allowed action | direct `npm publish` |

   Do not grant `npm dist-tag`. The workflow does not call `npm dist-tag`.
   Publishers created after 3 September 2026 allow `npm stage publish` until
   direct `npm publish` is also selected. Select direct `npm publish`.
   With npm CLI 11.15.0 or newer, logged in as the owner with account 2FA, the
   same configuration is one command per package. `--allow-publish` enables
   direct publication:

   ```sh
   npm trust github @matvei-radchenko/mcp-gate-bin-darwin-arm64 --file release.yml --repo Matvey-Radchenko/mcp-gate --env npm --allow-publish
   npm trust github @matvei-radchenko/mcp-gate-bin-darwin-x64 --file release.yml --repo Matvey-Radchenko/mcp-gate --env npm --allow-publish
   npm trust github @matvei-radchenko/mcp-gate-bin-win32-x64 --file release.yml --repo Matvey-Radchenko/mcp-gate --env npm --allow-publish
   npm trust github @matvei-radchenko/mcp-gate-bin-win32-arm64 --file release.yml --repo Matvey-Radchenko/mcp-gate --env npm --allow-publish
   npm trust github @matvei-radchenko/mcp-gate-bin-linux-x64 --file release.yml --repo Matvey-Radchenko/mcp-gate --env npm --allow-publish
   npm trust github @matvei-radchenko/mcp-gate-bin-linux-arm64 --file release.yml --repo Matvey-Radchenko/mcp-gate --env npm --allow-publish
   npm trust github @matvei-radchenko/mcp-gate-bin-linux-x64-musl --file release.yml --repo Matvey-Radchenko/mcp-gate --env npm --allow-publish
   npm trust github @matvei-radchenko/mcp-gate-bin-linux-arm64-musl --file release.yml --repo Matvey-Radchenko/mcp-gate --env npm --allow-publish
   npm trust github @matvei-radchenko/mcp-gate --file release.yml --repo Matvey-Radchenko/mcp-gate --env npm --allow-publish
   ```

   The first command prompts for 2FA. The website can skip 2FA for the next
   five minutes so the remaining commands can finish. The registry stores one
   trusted-publisher configuration per package. Replace an existing one with
   `npm trust list <package>` and `npm trust revoke <package> --id <id>` before
   creating another. No npm token belongs in the repository or workflow.
   The bootstrap release only verifies the owner-published tarballs. If their
   new trusted publishers expire, recreate them immediately before their first
   OIDC publish.
   0.1.0 was published by the owner from the downloaded archives. Those two
   unscoped Darwin packages were deprecated at that time:

   ```sh
   npm deprecate mcp-gate-bin-darwin-arm64 "Moved to @matvei-radchenko/mcp-gate-bin-darwin-arm64"
   npm deprecate mcp-gate-bin-darwin-x64 "Moved to @matvei-radchenko/mcp-gate-bin-darwin-x64"
   ```

7. The `registry` job downloads each GitHub archive, verifies its checksum, and
   installs `@matvei-radchenko/mcp-gate@<version>` for all eight targets. The
   Windows ARM64 runner is `windows-11-arm`. The musl checks run in Alpine
   containers on `ubuntu-24.04` and `ubuntu-24.04-arm`. The musl npm packages declare
   `libc: ["musl"]`, so npm installs them only on musl hosts. Glibc hosts get only
   the glibc package, and the musl archive install smoke runs only in Alpine. During CI, the static
   binaries also run the Linux user-service tests on Ubuntu because Alpine
   has no `systemd --user` manager. A 404 from either the new Release asset or
   the new scoped version is retried with
   backoff for a few minutes. Any other failure stops immediately. The release
   is not complete until these pass.
8. The Homebrew tap job is off. It runs only when the repository variable
   `HOMEBREW_TAP_ENABLED` is the string `true`. Leaving the variable unset skips
   the job, and that skip does not fail the release. The formula and tap notes
   stay in the repository for when Homebrew is enabled. The formula installs
   the macOS Release archives and has no `service do` block.

Updating npm or Homebrew alone changes no services. Users must run setup with the
chosen version to update or compatibly downgrade background gateways. The public
tag is `latest`; no public rollback/update/doctor command is introduced.

On 2026-09-09 the repository's `npm` environment was configured to require the
owner's approval and permit deployments only from `main`. This prepares the hold
needed for the first package bootstrap. It does not authenticate to npm, create
trusted publishers, publish a package or mark the acceptance gates as passed.
