import assert from 'node:assert/strict';
import test from 'node:test';
import { classify } from './ci-changes.mjs';

const scope = files => classify(files).scope;

test('documentation-only changes skip every heavy job', () => {
  const result = classify(['README.md', 'CHANGELOG.md', 'docs/usage.md', 'docs/assets/setup-demo.gif',
    'skills/mcp-gate/SKILL.md', '.github/ISSUE_TEMPLATE/bug.yml', 'release/notes.md', 'SECURITY.md']);
  assert.equal(result.scope, 'docs');
  assert.equal(result.launcher || result.native || result.docker, false);
});

test('each file group selects only the jobs it can affect', () => {
  assert.equal(scope(['release/acceptance.json']), 'scripts');
  assert.equal(scope(['npm/README.md']), 'launcher');
  assert.equal(scope(['LICENSE']), 'launcher');
  assert.equal(scope(['npm/mcp-gate/bin/mcp-gate.cjs']), 'native');
  assert.equal(scope(['packaging/build.mjs']), 'native');
  assert.equal(scope(['xtask/src/main.rs']), 'native');
  assert.equal(scope(['runtime/package-lock.json']), 'native');
  for (const path of ['src/main.rs', 'tests/shared.rs', 'tests/support/notes.md', 'Cargo.lock', 'build.rs',
    'recipes/shareability.json', '.github/workflows/ci.yml', 'packaging/test-docker-windows.ps1', '.gitattributes']) {
    assert.equal(scope([path]), 'full', path);
  }
});

test('the widest scope wins and unknown or empty changes run everything', () => {
  assert.deepEqual(classify(['README.md', 'npm/README.md']).launcher, true);
  assert.equal(classify(['README.md', 'npm/README.md']).native, false);
  assert.equal(scope(['README.md', 'src/lib.rs']), 'full');
  assert.equal(scope(['Dockerfile']), 'full');
  assert.equal(scope(['examples/demo.json']), 'full');
  assert.equal(scope([]), 'full');
  const full = classify(['src/lib.rs']);
  assert.equal(full.launcher && full.native && full.docker, true);
});
