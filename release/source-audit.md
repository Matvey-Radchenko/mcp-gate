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

## Follow-up acceptance and archive inspection

The 17 public commits through `a624d6e5db32ccfdad2736d7b17a8d87adfd5269` were
scanned with Gitleaks on 2026-09-09; no leaks were found. The original private
snapshot remains separate from the public history. The subsequent documentation
update records this evidence and clarifies that arbitrary configured commands
retain their own browser/profile behavior; it changes no release executable.

[Run 34336206965](https://github.com/Matvey-Radchenko/mcp-gate/actions/runs/34336206965)
passed all three native jobs, the shared-launcher build and combined-archive check.
All nine downloaded archive copies (seven unique archives) were inspected:
expected regular files only, native ARM64/x64/AMD64 architectures, binary SHA-256,
identical common launcher archives with executable mode, and no production test
root/failure hooks or unpublished compatibility binary. Known private home paths,
private-key blocks and npm/GitHub/API token signatures were absent. The 112 CI log
entries, approximately 737 KB of text, also passed the Gitleaks scan.

The earlier separate-platform archives had identical launcher file contents but
different executable tar modes on Windows and macOS. They are not the release
candidate. The common archive is now built once and its exact bytes are installed
on every native runner. Local/global command execution and the final combined
checksum check passed. A fresh release run still requires inspection of its own
artifacts before owner approval; this audit does not waive login, Docker or npm
owner-authorization gates.

## Windows cleanup candidate

[Run 34344798587](https://github.com/Matvey-Radchenko/mcp-gate/actions/runs/34344798587)
passed all five jobs at `ad36a4af72714a6c5f9b793fbaacebdc64b54e98`.
Its 20 reachable public commits and 112 CI log entries (approximately 742 KB)
passed Gitleaks inspection. All nine archive copies were inspected again with the
same exact file-list, architecture, binary-hash, shared-launcher and private-path
checks; no findings or production test hooks were found. Seven unique archives
passed the combined SHA-256 check. This candidate includes the Windows process
termination fix; it is still unpublished and does not waive the remaining gates.
