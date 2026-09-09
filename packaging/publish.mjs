// Resume an interrupted/owner-bootstrapped release only for identical npm bytes.
import { readFileSync } from 'node:fs';
import { join } from 'node:path';
import { createHash } from 'node:crypto';
import { spawnSync } from 'node:child_process';
import { pathToFileURL } from 'node:url';
import assert from 'node:assert/strict';

export function verifyPublished(bytes, metadata, name, version) {
  assert.equal(metadata.name, name, 'Registry package identity differs');
  assert.equal(metadata.version, version, 'Registry version differs');
  const integrity = `sha512-${createHash('sha512').update(bytes).digest('base64')}`;
  assert.equal(metadata.dist?.integrity, integrity,
    'Published version has different bytes; refusing to continue this release');
}

async function publish(root, version) {
  assert.match(version, /^\d+\.\d+\.\d+$/);
  const npm = process.env.npm_execpath;
  assert(npm, 'Run through npm exec -- node packaging/publish.mjs');
  const targets = ['darwin-arm64', 'darwin-x64', 'win32-x64'];
  const packages = targets.map(target => ({
    name: `mcp-gate-bin-${target}`, target,
  }));
  packages.push({ name: 'mcp-gate', target: 'darwin-arm64' });
  for (const { name, target } of packages) {
    const archive = join(root, `native-${target}`, `${name}-${version}.tgz`);
    const bytes = readFileSync(archive);
    const response = await fetch(`https://registry.npmjs.org/${name}/${version}`, {
      signal: AbortSignal.timeout(30_000),
    });
    if (response.status === 404) {
      const result = spawnSync(process.execPath, [npm, 'publish', archive,
        '--access', 'public', '--tag', 'latest', '--provenance'], { stdio: 'inherit' });
      assert.equal(result.status, 0, `Publication failed for ${name}; do not publish the launcher before every platform`);
    } else {
      assert(response.ok, `Cannot inspect registry version: ${response.status}`);
      verifyPublished(bytes, await response.json(), name, version);
      console.log(`Verified identical published archive: ${name}@${version}`);
    }
    const tags = await fetch(`https://registry.npmjs.org/-/package/${name}/dist-tags`, {
      signal: AbortSignal.timeout(30_000),
    });
    assert(tags.ok, `Cannot inspect dist-tags for ${name}`);
    assert.equal((await tags.json()).latest, version, `${name} latest differs; owner action required`);
  }
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  await publish(process.argv[2] ?? 'artifacts', process.argv[3]);
}
