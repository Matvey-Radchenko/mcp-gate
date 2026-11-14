import assert from 'node:assert/strict';
import { mkdtempSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { spawnSync } from 'node:child_process';
import test from 'node:test';
import { tar } from './download-smoke.mjs';

test('tar reads the archive by name inside its directory', () => {
  const directory = mkdtempSync(join(tmpdir(), 'mcp-gate tar Юникод '));
  const filename = 'mcp-gate-0.1.0-win32-x64.tar.gz';
  try {
    writeFileSync(join(directory, 'README.md'), 'mcp-gate\n');
    const packed = spawnSync('tar', ['-czf', filename, 'README.md'], {
      cwd: directory,
      encoding: 'utf8',
    });
    assert.equal(packed.status, 0, packed.stderr);
    // GNU tar treats a drive-letter colon as a remote host. The release check
    // must not hand tar that absolute form.
    const remote = spawnSync('tar', ['-tzf', `C:\\${filename}`], {
      cwd: directory,
      encoding: 'utf8',
      timeout: 5_000,
    });
    assert.notEqual(remote.status, 0, 'a C:\\ archive path must not succeed');
    const listing = tar(directory, ['-tzf', filename]);
    assert.equal(listing.status, 0, listing.stderr);
    assert.match(listing.stdout, /README\.md/);
    const extracted = tar(directory, ['-xzf', filename, '-C', '.']);
    assert.equal(extracted.status, 0, extracted.stderr);
  } finally {
    rmSync(directory, { recursive: true, force: true });
  }
});
