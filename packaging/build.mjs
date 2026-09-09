// Build only from a native release binary produced by the same CI job.
import { cpSync, mkdirSync, readFileSync, writeFileSync, chmodSync, realpathSync } from 'node:fs';
import { homedir } from 'node:os';
import { resolve, join } from 'node:path';
import { createHash } from 'node:crypto';
const target = `${process.platform}-${process.arch}`;
if (!['darwin-arm64', 'darwin-x64', 'win32-x64'].includes(target)) throw Error('Unsupported native target');
const manifest = JSON.parse(readFileSync('npm/mcp-gate/package.json', 'utf8'));
const cargoVersion = readFileSync('Cargo.toml', 'utf8').match(/^version = "([^"]+)"/m)?.[1];
if (cargoVersion !== manifest.version) throw Error('Cargo/npm version mismatch');
const executable = `mcp-gate${process.platform === 'win32' ? '.exe' : ''}`;
const binary = readFileSync(join('target/release', executable));
for (const path of [homedir(), process.cwd(), realpathSync(process.cwd()), process.env.CARGO_HOME, process.env.RUSTUP_HOME]) {
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
writeFileSync(join(native, 'package.json'), JSON.stringify({
  name: `mcp-gate-bin-${target}`, version: manifest.version, description: `Native mcp-gate for ${target}`,
  license: manifest.license, repository: manifest.repository, os: [process.platform], cpu: [process.arch],
  files: ['bin/', 'LICENSE', 'README.md'],
}, null, 2) + '\n');
cpSync('LICENSE', join(native, 'LICENSE'));
cpSync('npm/README.md', join(native, 'README.md'));
const hash = createHash('sha256').update(readFileSync(join(native, 'bin', executable))).digest('hex');
writeFileSync(join(output, `${target}.sha256`), `${hash}  ${executable}\n`);
console.log(`Prepared ${target} ${manifest.version}`);
