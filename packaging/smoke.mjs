import { mkdtempSync, readdirSync, readFileSync, rmSync } from 'node:fs';
import { join, resolve } from 'node:path';
import { tmpdir } from 'node:os';
import { spawnSync } from 'node:child_process';
import assert from 'node:assert/strict';
const directory = mkdtempSync(join(tmpdir(), 'mcp-gate clean Юникод '));
const target = `${process.platform}-${process.arch}`;
const version = JSON.parse(readFileSync('npm/mcp-gate/package.json')).version;
const npm = process.env.npm_execpath;
assert(npm, 'Run through npm exec -- node packaging/smoke.mjs');
function run(command, args, cwd = directory) {
  const result = spawnSync(command, args, { cwd, encoding: 'utf8', timeout: 120_000 });
  assert.equal(result.status, 0, result.stderr || result.error?.message);
  return result.stdout;
}
const packages = readdirSync('dist').filter(name => name.endsWith('.tgz')).map(name => resolve('dist', name));
assert.equal(packages.length, 2);
try {
  run(process.execPath, [npm, 'install', '--offline', '--ignore-scripts', '--no-audit', '--no-fund', '--no-package-lock', ...packages]);
  const launcher = join(directory, 'node_modules/mcp-gate/bin/mcp-gate.cjs');
  assert.equal(run(process.execPath, [launcher, '--version']).trim(), `mcp-gate ${version}`);
  const help = run(process.execPath, [launcher, '--help']);
  for (const command of ['setup', 'status', 'remove']) assert(help.includes(command));
  console.log(`Clean npm archive install passed: ${target}`);
} finally { rmSync(directory, { recursive: true, force: true }); }
