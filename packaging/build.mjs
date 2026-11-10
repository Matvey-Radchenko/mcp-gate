// Build only from a native release binary produced by the same CI job.
import { cpSync, mkdirSync, readFileSync, writeFileSync, chmodSync, realpathSync, statSync } from 'node:fs';
import { homedir } from 'node:os';
import { resolve, join } from 'node:path';
import { createHash } from 'node:crypto';
import { assertReleaseManifest, nativeTargets, platformPackage } from './manifest.mjs';

const target = `${process.platform}-${process.arch}`;
if (!nativeTargets.includes(target)) throw Error('Unsupported native target');
const manifest = JSON.parse(readFileSync('npm/mcp-gate/package.json', 'utf8'));
const cargoToml = readFileSync('Cargo.toml', 'utf8');
const launcherSource = readFileSync('npm/mcp-gate/bin/mcp-gate.cjs', 'utf8');
assertReleaseManifest(manifest, cargoToml, launcherSource);
const executable = `mcp-gate${process.platform === 'win32' ? '.exe' : ''}`;
const binary = readFileSync(join('target/release', executable));
const roots = [homedir(), process.cwd(), realpathSync(process.cwd()), process.env.CARGO_HOME, process.env.RUSTUP_HOME];
for (const path of roots) {
  if (path && [path, path.replaceAll('\\', '/')].some(value => binary.includes(Buffer.from(value)))) {
    throw Error('Native binary retains build-machine paths. Rebuild with node packaging/compile.mjs.');
  }
}
const output = resolve('dist');
mkdirSync(output, { recursive: true });
const native = join(output, `mcp-gate-bin-${target}`);
mkdirSync(join(native, 'bin'), { recursive: true });
cpSync(join('target/release', executable), join(native, 'bin', executable));
chmodSync(join(native, 'bin', executable), 0o755);
const packageJson = `${JSON.stringify(platformPackage(manifest, target), null, 2)}\n`;
writeFileSync(join(native, 'package.json'), packageJson);
cpSync('LICENSE', join(native, 'LICENSE'));
cpSync('npm/README.md', join(native, 'README.md'));
for (const name of ['LICENSE', 'README.md']) {
  if (statSync(join(native, name)).size === 0) throw Error(`Staged ${name} is empty`);
}
const hash = createHash('sha256').update(readFileSync(join(native, 'bin', executable))).digest('hex');
writeFileSync(join(output, `${target}.sha256`), `${hash}  ${executable}\n`);
console.log(`Prepared ${target} ${manifest.version}`);
