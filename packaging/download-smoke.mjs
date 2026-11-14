import { mkdtempSync, writeFileSync, rmSync, mkdirSync } from 'node:fs';
import { join } from 'node:path';
import { tmpdir } from 'node:os';
import { createHash } from 'node:crypto';
import { spawnSync } from 'node:child_process';
import { pathToFileURL } from 'node:url';
import assert from 'node:assert/strict';

// Git Bash ships GNU tar, which treats `C:\...` as host `C`. Archive names
// passed here are relative to `directory`, so the colon never reaches tar.
export function tar(directory, args) {
  return spawnSync('tar', args, { cwd: directory, encoding: 'utf8' });
}

async function download(base, name) {
  const response = await fetch(`${base}/${name}`);
  assert(response.ok, `Download failed: ${name} ${response.status}`);
  return Buffer.from(await response.arrayBuffer());
}

async function verifyRelease(version) {
  assert.match(version, /^\d+\.\d+\.\d+$/);
  const target = `${process.platform}-${process.arch}`;
  assert(['darwin-arm64', 'darwin-x64', 'win32-x64'].includes(target));
  const filename = `mcp-gate-${version}-${target}.tar.gz`;
  const base = `https://github.com/Matvey-Radchenko/mcp-gate/releases/download/v${version}`;
  const directory = mkdtempSync(join(tmpdir(), 'mcp-gate release Юникод '));
  try {
    const sums = (await download(base, 'SHA256SUMS')).toString('utf8');
    const digest = sums.split('\n').find(line => line.endsWith(`  ${filename}`))?.split('  ')[0];
    assert.match(digest ?? '', /^[0-9a-f]{64}$/);
    const archive = await download(base, filename);
    assert.equal(createHash('sha256').update(archive).digest('hex'), digest);
    writeFileSync(join(directory, filename), archive);
    const listing = tar(directory, ['-tzf', filename]);
    assert.equal(listing.status, 0, listing.stderr);
    const exe = `mcp-gate${process.platform === 'win32' ? '.exe' : ''}`;
    const allow = new Set(['bin/', 'bin', `bin/${exe}`, 'README.md', 'LICENSE']);
    for (const name of listing.stdout.trim().split(/\r?\n/)) {
      assert(allow.has(name), `Unexpected archive entry: ${name}`);
    }
    const expanded = 'expanded';
    mkdirSync(join(directory, expanded));
    const extracted = tar(directory, ['-xzf', filename, '-C', expanded]);
    assert.equal(extracted.status, 0, extracted.stderr);
    const result = spawnSync(join(directory, expanded, 'bin', exe), ['--version'], {
      encoding: 'utf8',
      timeout: 15_000,
    });
    assert.equal(result.status, 0, result.stderr);
    assert.equal(result.stdout.trim(), `mcp-gate ${version}`);
    console.log(`Verified downloaded GitHub archive: ${target}`);
  } finally {
    rmSync(directory, { recursive: true, force: true });
  }
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  await verifyRelease(process.argv[2]);
}
