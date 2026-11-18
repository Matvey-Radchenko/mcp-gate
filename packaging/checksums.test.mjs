import { createHash } from 'node:crypto';
import { mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join, resolve } from 'node:path';
import { spawnSync } from 'node:child_process';
import assert from 'node:assert/strict';
import test from 'node:test';
import { launcherPackage, nativeTargets, npmTarball, platformPackageName } from './manifest.mjs';

test('combined release accepts one shared launcher and rejects conflicting or missing archives', () => {
  const root = mkdtempSync(join(tmpdir(), 'mcp-gate checksums '));
  const rows = new Map();
  function archive(target, name, bytes = Buffer.from(name)) {
    writeFileSync(join(root, `native-${target}`, name), bytes);
    rows.set(name, createHash('sha256').update(bytes).digest('hex'));
  }
  const launcher = npmTarball(launcherPackage, '0.1.0');
  const checksums = join(root, 'SHA256SUMS');
  const verify = () => spawnSync(process.execPath, [resolve('packaging/checksums.mjs'), root], { encoding: 'utf8' });
  try {
    for (const target of nativeTargets) {
      mkdirSync(join(root, `native-${target}`));
      archive(target, launcher);
      archive(target, npmTarball(platformPackageName(target), '0.1.0'));
      archive(target, `mcp-gate-0.1.0-${target}.tar.gz`);
    }
    const combined = verify();
    assert.equal(combined.status, 0, combined.stderr);
    assert.equal(rows.size, 11);
    const expected = [...rows]
      .filter(([name]) => name.endsWith('.tar.gz'))
      .sort(([a], [b]) => a.localeCompare(b))
      .map(([name, hash]) => `${hash}  ${name}\n`).join('');
    assert.equal(readFileSync(checksums, 'utf8'), expected);
    assert.equal(expected.trimEnd().split('\n').length, 5);
    assert.doesNotMatch(readFileSync(checksums, 'utf8'), /\.tgz/);

    archive('win32-x64', launcher, Buffer.from('different tar permissions'));
    const conflict = verify();
    assert.notEqual(conflict.status, 0);
    assert.match(conflict.stderr, /Duplicate archive differs/);
    assert.equal(readFileSync(checksums, 'utf8'), expected);

    archive('win32-x64', launcher);
    rmSync(join(root, 'native-linux-arm64', 'mcp-gate-0.1.0-linux-arm64.tar.gz'));
    const missing = verify();
    assert.notEqual(missing.status, 0);
    assert.match(missing.stderr, /Expected 6 npm packages and 5 native archives; found 10 files \(6 npm packages and 4 native archives\)/);
    assert.equal(readFileSync(checksums, 'utf8'), expected);
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});
