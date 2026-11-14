import test from 'node:test';
import assert from 'node:assert/strict';
import { createHash } from 'node:crypto';
import { publishCommand, publishEnvironment, verifyPublished } from './publish.mjs';

test('release resume accepts identical bytes but rejects a different immutable package', () => {
  const bytes = Buffer.from('reviewed fixture archive');
  const published = {
    name: '@matvei-radchenko/mcp-gate', version: '0.1.0',
    dist: { integrity: `sha512-${createHash('sha512').update(bytes).digest('base64')}` },
  };
  verifyPublished(bytes, published, '@matvei-radchenko/mcp-gate', '0.1.0');
  assert.throws(() => verifyPublished(Buffer.from('other build'), published, '@matvei-radchenko/mcp-gate', '0.1.0'));
  assert.throws(() => verifyPublished(bytes, published, '@matvei-radchenko/mcp-gate-bin-win32-x64', '0.1.0'));
  assert.throws(() => verifyPublished(bytes, published, '@matvei-radchenko/mcp-gate', '0.2.0'));
  assert.throws(() => verifyPublished(bytes, { ...published, dist: {} }, '@matvei-radchenko/mcp-gate', '0.1.0'));
  assert.deepEqual(publishCommand('/tmp/mcp-gate-bin.tgz'), [
    'publish', '/tmp/mcp-gate-bin.tgz', '--access', 'public', '--tag', 'latest', '--provenance',
  ]);
  const env = publishEnvironment({ NODE_AUTH_TOKEN: 'secret', NPM_TOKEN: 'secret', PATH: '/usr/bin' });
  assert.equal(env.NODE_AUTH_TOKEN, undefined);
  assert.equal(env.NPM_TOKEN, undefined);
  assert.equal(env.PATH, '/usr/bin');
});
