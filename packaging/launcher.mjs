// Produce the shared launcher once, then test these exact bytes on every target.
import { chmodSync, cpSync, mkdirSync, readFileSync } from 'node:fs';
import { join } from 'node:path';

const manifest = JSON.parse(readFileSync('npm/mcp-gate/package.json', 'utf8'));
const cargoVersion = readFileSync('Cargo.toml', 'utf8').match(/^version = "([^"]+)"/m)?.[1];
if (cargoVersion !== manifest.version) throw Error('Cargo/npm version mismatch');
const launcher = join('dist', 'mcp-gate');
mkdirSync('dist', { recursive: true });
cpSync('npm/mcp-gate', launcher, { recursive: true });
chmodSync(join(launcher, 'bin/mcp-gate.cjs'), 0o755);
cpSync('LICENSE', join(launcher, 'LICENSE'));
cpSync('npm/README.md', join(launcher, 'README.md'));
console.log(`Prepared shared launcher ${manifest.version}`);
