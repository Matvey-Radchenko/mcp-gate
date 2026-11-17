import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { copyFileSync, mkdirSync, mkdtempSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import test from 'node:test';

for (const longPath of [false, true]) {
  test(`Windows launcher forwards arguments and exit status from a ${longPath ? 'long' : 'short'} native path`, {
    skip: process.platform !== 'win32' || process.arch !== 'x64',
  }, t => {
    const temporary = mkdtempSync(join(tmpdir(), 'mcp-gate-launcher-'));
    t.after(() => rmSync(temporary, { recursive: true, force: true }));
    const root = join(temporary, 'space and юникод', ...(longPath ? ['nested-'.repeat(10), 'cache-'.repeat(10)] : []));
    const launcher = join(root, 'bin', 'mcp-gate.cjs');
    const binary = join(root, 'node_modules', '@matvei-radchenko', 'mcp-gate-bin-win32-x64', 'bin', 'mcp-gate.exe');
    mkdirSync(join(root, 'bin'), { recursive: true });
    mkdirSync(join(binary, '..'), { recursive: true });
    copyFileSync(new URL('../npm/mcp-gate/package.json', import.meta.url), join(root, 'package.json'));
    copyFileSync(new URL('../npm/mcp-gate/bin/mcp-gate.cjs', import.meta.url), launcher);
    // Node is a real native executable, so this checks CreateProcess rather than a mocked spawn
    copyFileSync(process.execPath, binary);
    assert.equal(binary.length > 260, longPath);
    const argumentsToForward = ['space value', 'quote"value', 'юникод'];
    const result = spawnSync(process.execPath, [
      launcher, '-e', 'console.log(JSON.stringify(process.argv.slice(1))); process.exitCode = 7;',
      ...argumentsToForward,
    ], { encoding: 'utf8', windowsHide: true, timeout: 30_000 });
    assert.equal(result.error, undefined, result.error?.message);
    assert.equal(result.status, 7, result.stderr);
    assert.equal(result.stderr, '');
    assert.deepEqual(JSON.parse(result.stdout), argumentsToForward);
  });
}
