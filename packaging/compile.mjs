// Keep developer home/workspace paths out of distributed panic locations.
import { realpathSync } from 'node:fs';
import { homedir } from 'node:os';
import { spawnSync } from 'node:child_process';

const flags = process.env.CARGO_ENCODED_RUSTFLAGS !== undefined
  ? process.env.CARGO_ENCODED_RUSTFLAGS.split('\x1f').filter(Boolean)
  : (process.env.RUSTFLAGS ?? '').split(/\s+/).filter(Boolean);
const mappings = [
  [homedir(), '/build/home'],
  [process.env.CARGO_HOME, '/build/cargo'],
  [process.env.RUSTUP_HOME, '/build/rustup'],
  [process.cwd(), '/build/mcp-gate'],
  [realpathSync(process.cwd()), '/build/mcp-gate'],
];
for (const [source, destination] of mappings) {
  if (source) {
    for (const spelling of new Set([source, source.replaceAll('\\', '/')])) {
      flags.push(`--remap-path-prefix=${spelling}=${destination}`);
    }
  }
}
const result = spawnSync('cargo', ['build', '--locked', '--release', '--bin', 'mcp-gate'], {
  stdio: 'inherit',
  env: { ...process.env, CARGO_ENCODED_RUSTFLAGS: flags.join('\x1f') },
});
if (result.error) console.error(result.error.message);
process.exitCode = result.status ?? 1;
