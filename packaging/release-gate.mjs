import { readFileSync } from 'node:fs';
import assert from 'node:assert/strict';
const manifest=JSON.parse(readFileSync('npm/mcp-gate/package.json'));
const acceptance=JSON.parse(readFileSync('release/acceptance.json'));
const version=process.argv[2];
assert.match(version, /^\d+\.\d+\.\d+$/);
assert.equal(manifest.version,version);
assert.equal(acceptance.version,version);
const required=['native-matrix','clean-service-login','safe-update-downgrade',
  'failure-recovery-and-concurrent-edits','multi-client-remove','browser-isolation-artifacts-cleanup',
  'windows-cmd-npx-uvx-docker','source-history-and-archive-inspection'];
for (const id of required) {
  const check=acceptance.checks.find(check=>check.id===id);
  assert(check?.status==='passed' && check.evidence?.length>0, `Release gate is incomplete: ${id}`);
}
console.log(`Release evidence gate passed for ${version}`);
