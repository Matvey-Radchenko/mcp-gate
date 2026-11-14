import assert from 'node:assert/strict';
import test from 'node:test';
import { notFoundWaitsMs } from './not-found-retry.mjs';
import { downloadAsset } from './download-smoke.mjs';
import { installPublished } from './registry-install.mjs';

test('a new release asset is retried only while the response is 404', async () => {
  const waits = [];
  let calls = 0;
  const bytes = Buffer.from('archive');
  const received = await downloadAsset('https://example.test/mcp-gate.tar.gz', async () => {
    calls += 1;
    if (calls < 3) return { ok: false, status: 404 };
    return { ok: true, status: 200, arrayBuffer: async () => bytes };
  }, ms => { waits.push(ms); });
  assert.equal(calls, 3);
  assert.deepEqual(waits, notFoundWaitsMs.slice(0, 2));
  assert.equal(received.toString(), 'archive');
  let other = 0;
  await assert.rejects(
    () => downloadAsset('https://example.test/missing', async () => {
      other += 1;
      return { ok: false, status: 500 };
    }, () => { throw new Error('a non-404 must not wait'); }),
    /Download failed: 500/,
  );
  assert.equal(other, 1);
});

test('npm install retries a 404 and fails immediately on any other error', async () => {
  const waits = [];
  let calls = 0;
  const stdout = await installPublished('0.1.1', async () => {
    calls += 1;
    if (calls < 3) return { status: 1, stderr: 'npm error code E404\n404 Not Found' };
    return { status: 0, stdout: 'mcp-gate 0.1.1\n' };
  }, ms => { waits.push(ms); });
  assert.equal(stdout, 'mcp-gate 0.1.1\n');
  assert.deepEqual(waits, notFoundWaitsMs.slice(0, 2));
  await assert.rejects(
    () => installPublished('0.1.1', async () => ({ status: 1, stderr: 'npm error code ETARGET' }), () => {
      throw new Error('a non-404 must not wait');
    }),
    /ETARGET/,
  );
});
