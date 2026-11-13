// Ship only with complete evidence. Pending never passes.
// passed-with-limitations is allowed only while the limitations stay written down.
import { readFileSync } from 'node:fs';
import assert from 'node:assert/strict';
import { pathToFileURL } from 'node:url';

export const requiredChecks = [
  'native-matrix',
  'clean-service-login',
  'macos-protected-folder',
  'safe-update-downgrade',
  'failure-recovery-and-concurrent-edits',
  'multi-client-remove',
  'browser-isolation-artifacts-cleanup',
  'windows-cmd-npx-uvx-docker',
  'source-history-and-archive-inspection',
];

function texts(value) {
  if (typeof value === 'string') return value.trim() === '' ? [] : [value.trim()];
  if (!Array.isArray(value)) return [];
  return value.filter(item => typeof item === 'string' && item.trim() !== '');
}

function assertCheck(check, id) {
  assert(check, `Release gate is missing: ${id}`);
  const evidence = texts(check.evidence);
  assert(Array.isArray(check.evidence) && evidence.length > 0 && evidence.length === check.evidence.length,
    `Release gate evidence is incomplete: ${id}`);
  const limitations = [...texts(check.remaining), ...texts(check.limitations)];
  if (check.status === 'passed') {
    assert.equal(limitations.length, 0, `Release gate cannot pass ${id} while limitations remain`);
    return;
  }
  assert.equal(check.status, 'passed-with-limitations', `Release gate is incomplete: ${id}`);
  assert(limitations.length > 0, `Release gate limitations are missing: ${id}`);
}

export function assertReleaseEvidence(manifest, acceptance, version) {
  assert.match(String(version ?? ''), /^\d+\.\d+\.\d+$/);
  assert.equal(manifest.version, version);
  assert.equal(acceptance.version, version);
  for (const id of requiredChecks) {
    assertCheck(acceptance.checks.find(item => item.id === id), id);
  }
}

function main() {
  const version = process.argv[2];
  const manifest = JSON.parse(readFileSync('npm/mcp-gate/package.json', 'utf8'));
  const acceptance = JSON.parse(readFileSync('release/acceptance.json', 'utf8'));
  assertReleaseEvidence(manifest, acceptance, version);
  console.log(`Release evidence gate passed for ${version}`);
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) main();
