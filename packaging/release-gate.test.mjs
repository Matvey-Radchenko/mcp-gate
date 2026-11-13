import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import test from 'node:test';
import { assertReleaseEvidence } from './release-gate.mjs';

const manifest = { version: '0.1.0' };
const ids = [
  'native-matrix', 'clean-service-login', 'macos-protected-folder', 'safe-update-downgrade',
  'failure-recovery-and-concurrent-edits', 'multi-client-remove', 'browser-isolation-artifacts-cleanup',
  'windows-cmd-npx-uvx-docker', 'source-history-and-archive-inspection',
];

function acceptance(overrides = {}) {
  return {
    version: '0.1.0',
    checks: ids.map(id => ({ id, status: 'passed', evidence: [`${id} evidence`], ...overrides[id] })),
  };
}

test('0.1.0 evidence ships login and Documents access only with the recorded limitations', () => {
  const realManifest = JSON.parse(readFileSync('npm/mcp-gate/package.json', 'utf8'));
  const real = JSON.parse(readFileSync('release/acceptance.json', 'utf8'));
  assertReleaseEvidence(realManifest, real, '0.1.0');
  const login = real.checks.find(check => check.id === 'clean-service-login');
  const folder = real.checks.find(check => check.id === 'macos-protected-folder');
  assert.equal(login.status, 'passed-with-limitations');
  assert.equal(folder.status, 'passed-with-limitations');
  const loginText = login.remaining.join('\n');
  assert.match(loginText, /native Intel/);
  assert.match(loginText, /native Windows/);
  assert.match(loginText, /Wine/);
  assert.match(loginText, /Rosetta/);
  assert.match(folder.remaining.join('\n'), /first-prompt/);
  assert.match(folder.remaining.join('\n'), /deny/);
  assert.match(login.evidence.join('\n'), /arm64-reboot-2026-09-09\.json/);
  assert.match(login.evidence.join('\n'), /windows-whisky-smoke-2026-11-13\.json/);
  assert.match(folder.evidence.join('\n'), /tcc-documents-already-granted-2026-11-13\.json/);
});

test('pending never passes, and limitations require their own status', () => {
  assert.throws(() => assertReleaseEvidence(manifest, acceptance({
    'clean-service-login': { status: 'pending', evidence: ['noted'], remaining: ['still open'] },
  }), '0.1.0'), /incomplete: clean-service-login/);
  assert.throws(() => assertReleaseEvidence(manifest, acceptance({
    'macos-protected-folder': { status: 'passed', evidence: ['noted'], remaining: ['still open'] },
  }), '0.1.0'), /cannot pass macos-protected-folder/);
  assert.throws(() => assertReleaseEvidence(manifest, acceptance({
    'clean-service-login': { status: 'passed-with-limitations', evidence: ['noted'], remaining: [] },
  }), '0.1.0'), /limitations are missing: clean-service-login/);
  assert.throws(() => assertReleaseEvidence(manifest, acceptance({
    'clean-service-login': { status: 'passed-with-limitations', evidence: ['noted'], remaining: ['  '] },
  }), '0.1.0'), /limitations are missing: clean-service-login/);
  assert.throws(() => assertReleaseEvidence(manifest, acceptance({
    'clean-service-login': { status: 'passed-with-limitations', evidence: [] , limitations: ['still open'] },
  }), '0.1.0'), /evidence is incomplete: clean-service-login/);
  const withLimitations = acceptance({
    'macos-protected-folder': { status: 'passed-with-limitations', evidence: ['observed'], limitations: ['deny path not observed'] },
  });
  assertReleaseEvidence(manifest, withLimitations, '0.1.0');
});
