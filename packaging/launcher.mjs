// Produce the shared launcher once, then test these exact bytes on every target.
import { chmodSync, cpSync, mkdirSync, readFileSync, statSync } from 'node:fs';
import { join } from 'node:path';
import { assertReleaseManifest } from './manifest.mjs';

const manifest = JSON.parse(readFileSync('npm/mcp-gate/package.json', 'utf8'));
const cargoToml = readFileSync('Cargo.toml', 'utf8');
const launcherSource = readFileSync('npm/mcp-gate/bin/mcp-gate.cjs', 'utf8');
assertReleaseManifest(manifest, cargoToml, launcherSource);
const launcher = join('dist', 'mcp-gate');
mkdirSync('dist', { recursive: true });
cpSync('npm/mcp-gate', launcher, { recursive: true });
chmodSync(join(launcher, 'bin/mcp-gate.cjs'), 0o755);
cpSync('LICENSE', join(launcher, 'LICENSE'));
cpSync('npm/README.md', join(launcher, 'README.md'));
for (const name of ['README.md', 'LICENSE']) {
  if (statSync(join(launcher, name)).size === 0) throw Error(`Staged launcher ${name} is empty`);
}
console.log(`Prepared shared launcher ${manifest.version}`);
