import test from 'node:test';
import assert from 'node:assert/strict';
import { createHash } from 'node:crypto';
import { verifyPublished } from './publish.mjs';

test('release resume accepts identical bytes but rejects a different immutable package', () => {
  const bytes = Buffer.from('reviewed fixture archive');
  const published = {
    name: 'mcp-gate', version: '0.1.0',
    dist: { integrity: `sha512-${createHash('sha512').update(bytes).digest('base64')}` },
  };
  verifyPublished(bytes, published, 'mcp-gate', '0.1.0');
  assert.throws(() => verifyPublished(Buffer.from('other build'), published, 'mcp-gate', '0.1.0'));
  assert.throws(() => verifyPublished(bytes, published, 'mcp-gate-bin-win32-x64', '0.1.0'));
  assert.throws(() => verifyPublished(bytes, published, 'mcp-gate', '0.2.0'));
  assert.throws(() => verifyPublished(bytes, { ...published, dist: {} }, 'mcp-gate', '0.1.0'));
});
