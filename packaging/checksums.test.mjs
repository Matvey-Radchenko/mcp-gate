import { createHash } from 'node:crypto';
import { mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join, resolve } from 'node:path';
import { spawnSync } from 'node:child_process';
import assert from 'node:assert/strict';
import test from 'node:test';

test('combined release accepts one shared launcher and rejects conflicting or missing archives', () => {
  const root = mkdtempSync(join(tmpdir(), 'mcp-gate checksums '));
  const rows = new Map();
  const targets = ['darwin-arm64', 'darwin-x64', 'win32-x64'];
  function archive(target, name, bytes = Buffer.from(name)) {
    writeFileSync(join(root, `native-${target}`, name), bytes);
    rows.set(name, createHash('sha256').update(bytes).digest('hex'));
  }
  const launcher = 'mcp-gate-0.1.0.tgz';
  const checksums = join(root, 'SHA256SUMS');
  const verify = () => spawnSync(process.execPath, [resolve('packaging/checksums.mjs'), root], { encoding: 'utf8' });
  try {
    for (const target of targets) {
      mkdirSync(join(root, `native-${target}`));
      archive(target, launcher);
      archive(target, `mcp-gate-bin-${target}-0.1.0.tgz`);
      archive(target, `mcp-gate-0.1.0-${target}.tar.gz`);
    }
    assert.equal(verify().status, 0);
    const expected = [...rows].sort(([a], [b]) => a.localeCompare(b))
      .map(([name, hash]) => `${hash}  ${name}\n`).join('');
    assert.equal(readFileSync(checksums, 'utf8'), expected);

    archive('win32-x64', launcher, Buffer.from('different tar permissions'));
    const conflict = verify();
    assert.notEqual(conflict.status, 0);
    assert.match(conflict.stderr, /Duplicate archive differs/);
    assert.equal(readFileSync(checksums, 'utf8'), expected);

    archive('win32-x64', launcher);
    rmSync(join(root, 'native-win32-x64', 'mcp-gate-0.1.0-win32-x64.tar.gz'));
    const missing = verify();
    assert.notEqual(missing.status, 0);
    assert.match(missing.stderr, /Expected four npm packages and three native archives/);
    assert.equal(readFileSync(checksums, 'utf8'), expected);
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});
