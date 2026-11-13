import { existsSync, mkdirSync, mkdtempSync, readdirSync, readFileSync, rmSync } from 'node:fs';
import { delimiter, join, resolve } from 'node:path';
import { tmpdir } from 'node:os';
import { spawnSync } from 'node:child_process';
import assert from 'node:assert/strict';
const directory = mkdtempSync(join(tmpdir(), 'mcp-gate clean Юникод '));
const target = `${process.platform}-${process.arch}`;
const version = JSON.parse(readFileSync('npm/mcp-gate/package.json')).version;
const npm = process.env.npm_execpath;
assert(npm, 'Run through npm exec -- node packaging/smoke.mjs');
const local = join(directory, 'local');
const prefix = join(directory, 'global');
const globalRun = join(directory, 'global-run');
function run(command, args, cwd = local, env = process.env) {
  const result = spawnSync(command, args, { cwd, env, encoding: 'utf8', timeout: 120_000 });
  assert.equal(result.status, 0, result.stderr || result.error?.message);
  return result.stdout;
}
const packages = readdirSync('dist').filter(name => name.endsWith('.tgz')).map(name => resolve('dist', name));
assert.equal(packages.length, 2);
try {
  mkdirSync(local);
  mkdirSync(globalRun);
  run(process.execPath, [npm, 'install', '--offline', '--ignore-scripts', '--no-audit', '--no-fund', '--no-package-lock', ...packages]);
  // Exercise npm's real bin link / Windows .cmd shim, including executable mode.
  assert.equal(run(process.execPath, [npm, 'exec', '--offline', '--', 'mcp-gate', '--version']).trim(), `mcp-gate ${version}`);
  const help = run(process.execPath, [npm, 'exec', '--offline', '--', 'mcp-gate', '--help']);
  for (const command of ['setup', 'status', 'remove']) assert(help.includes(command));

  run(process.execPath, [npm, 'install', '--global', '--prefix', prefix, '--offline', '--ignore-scripts', '--no-audit', '--no-fund', ...packages]);
  const globalBin = process.platform === 'win32' ? prefix : join(prefix, 'bin');
  assert(existsSync(join(globalBin, process.platform === 'win32' ? 'mcp-gate.cmd' : 'mcp-gate')));
  const pathKey = Object.keys(process.env).find(key => key.toLowerCase() === 'path') ?? 'PATH';
  const env = { ...process.env, [pathKey]: `${globalBin}${delimiter}${process.env[pathKey] ?? ''}` };
  // An unrelated cwd has no local install that could mask a broken global link.
  assert.equal(run(process.execPath, [npm, 'exec', '--offline', '--call', 'mcp-gate --version'], globalRun, env).trim(), `mcp-gate ${version}`);
  console.log(`Clean npm archive install and local/global command launch passed: ${target}`);
} finally { rmSync(directory, { recursive: true, force: true }); }
