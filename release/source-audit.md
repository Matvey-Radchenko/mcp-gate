# Source publication audit — 2026-09-09

The owner authorized making this repository public before completing release
acceptance. The repository is public; npm 0.1.0 and a GitHub Release remain
unpublished. Publishing source does not mark the release gates as passed.

Reviewed remote refs:

- `main`: `a0b0e351cb49650974b8e54b517340bbfa9ba842`
- `codex/native-acceptance`: `1926949aa5f9182b759cfcfc25c848a426d441c5`

Both refs start at the sanitized root `7e4512f`. The ten reachable commits contain
198 unique file blobs. Gitleaks found no secrets in that history. A separate scan
found no known private home paths, private-key blocks or npm/GitHub/API token
signatures. Commit attribution uses the owner's GitHub noreply address. URL
literals, filenames, recipes and the existing PR description were reviewed.
There were no additional remote branches, tags, releases or discussion comments.

All eleven existing Actions runs and eleven retained native artifact archives
were downloaded for inspection. The two billing-blocked runs had empty log
archives. Gitleaks found no secrets in the remaining 2.29 MB of log text. Archives,
including nested npm packages and native binaries, passed the targeted private
path and token-signature scan. Original personal operational material and local
history are not reachable from the published refs.

These are inspection results, not a guarantee that pattern matching detects every
secret. Final release artifacts and subsequent commits still require inspection.
