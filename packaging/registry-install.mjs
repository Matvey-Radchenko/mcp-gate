// Install the just-published launcher. A 404 is registry lag, not a failed release.
import { spawnSync } from 'node:child_process';
import { pathToFileURL } from 'node:url';
import assert from 'node:assert/strict';
import { launcherPackage } from './manifest.mjs';
import { retryNotFound } from './not-found-retry.mjs';

export async function installPublished(version, run, sleep) {
  assert.match(String(version ?? ''), /^\d+\.\d+\.\d+$/);
  const spec = `${launcherPackage}@${version}`;
  return retryNotFound(async () => {
    const result = await run(spec);
    if (result.status === 0) {
      const text = result.stdout ?? '';
      assert.match(text, new RegExp(`mcp-gate ${version.replaceAll('.', '\\.')}`));
      return text;
    }
    const detail = `${result.stderr ?? ''}\n${result.stdout ?? ''}`.trim();
    const error = new Error(detail || `Install failed with status ${result.status}`);
    error.notFound = /\b404\b|E404/.test(detail);
    throw error;
  }, sleep);
}

function spawnNpx(spec) {
  const command = process.platform === 'win32' ? 'npx.cmd' : 'npx';
  return spawnSync(command, ['--yes', spec, '--version'], {
    encoding: 'utf8',
    timeout: 180_000,
    shell: process.platform === 'win32',
  });
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  const stdout = await installPublished(process.argv[2], spec => spawnNpx(spec));
  process.stdout.write(stdout.endsWith('\n') ? stdout : `${stdout}\n`);
}
