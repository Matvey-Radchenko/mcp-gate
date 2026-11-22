// Keep developer home/workspace paths out of distributed panic locations.
import { copyFileSync, mkdirSync, realpathSync } from 'node:fs';
import { homedir } from 'node:os';
import { spawnSync } from 'node:child_process';
import { join } from 'node:path';
import { nativeBuildTarget, staticRustTarget } from './manifest.mjs';

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
const args = ['build', '--locked', '--release', '--bin', 'mcp-gate', '--target-dir', 'target'];
const rustTarget = staticRustTarget(nativeBuildTarget());
if (rustTarget) {
  args.push('--target', rustTarget);
  flags.push('-Ctarget-feature=+crt-static');
}
const result = spawnSync('cargo', args, {
  stdio: 'inherit',
  env: { ...process.env, CARGO_ENCODED_RUSTFLAGS: flags.join('\x1f') },
});
if (result.error) console.error(result.error.message);
if (result.status === 0 && rustTarget) {
  const executable = process.platform === 'win32' ? 'mcp-gate.exe' : 'mcp-gate';
  mkdirSync('target/release', { recursive: true });
  copyFileSync(join('target', rustTarget, 'release', executable), join('target/release', executable));
}
process.exitCode = result.status ?? 1;
