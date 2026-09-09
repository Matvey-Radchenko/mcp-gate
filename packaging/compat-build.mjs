// Build a separately versioned, unpublished compatibility fixture from this tree.
// Its executable is never staged in dist or selected by the npm launcher.
import { cpSync, mkdirSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { resolve, join } from 'node:path';
import { spawnSync } from 'node:child_process';
import assert from 'node:assert/strict';

const root = resolve('.');
const directory = join(root, 'target/compatibility-source');
const target = join(root, 'target/compatibility');
const version = JSON.parse(readFileSync('npm/mcp-gate/package.json')).version;
assert.match(version, /^\d+\.\d+\.\d+$/);
rmSync(directory, { recursive: true, force: true });
mkdirSync(directory, { recursive: true });
for (const name of ['Cargo.toml', 'Cargo.lock', 'src', 'xtask', 'recipes']) {
  cpSync(join(root, name), join(directory, name), { recursive: true });
}
const fixtureVersion = `${version}-compat-fixture`;
for (const name of ['Cargo.toml', 'Cargo.lock']) {
  const path = join(directory, name);
  const before = readFileSync(path, 'utf8');
  const needle = `name = "mcp-gate"\nversion = "${version}"`;
  assert.equal(before.split(needle).length, 2, `Ambiguous package version in ${name}`);
  writeFileSync(path, before.replace(needle, `name = "mcp-gate"\nversion = "${fixtureVersion}"`));
}
const args = ['build', '--locked', '--release', '--bin', 'mcp-gate',
  '--manifest-path', join(directory, 'Cargo.toml'), '--target-dir', target];
const env = { ...process.env };
if (process.platform === 'win32') {
  args.push('--target', 'x86_64-pc-windows-msvc');
  env.CARGO_ENCODED_RUSTFLAGS = [
    ...(env.CARGO_ENCODED_RUSTFLAGS?.split('\x1f') ?? (env.RUSTFLAGS ?? '').split(/\s+/)),
    '-Ctarget-feature=+crt-static',
  ].filter(Boolean).join('\x1f');
}
const result = spawnSync('cargo', args, { stdio: 'inherit', env });
assert.equal(result.status, 0, result.error?.message ?? 'Compatibility fixture compilation failed');
const executable = join(target, ...(process.platform === 'win32'
  ? ['x86_64-pc-windows-msvc', 'release', 'mcp-gate.exe'] : ['release', 'mcp-gate']));
const checked = spawnSync(executable, ['--version'], { encoding: 'utf8' });
assert.equal(checked.status, 0, checked.stderr);
assert.equal(checked.stdout.trim(), `mcp-gate ${fixtureVersion}`);
console.log(`Unpublished compatibility fixture ready: ${fixtureVersion}`);
