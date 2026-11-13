import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import test from 'node:test';
import { renderHomebrewFormula } from './homebrew.mjs';

const template = readFileSync('packaging/homebrew/mcp-gate.rb.tmpl', 'utf8');
const arm = 'b'.repeat(64);
const intel = 'c'.repeat(64);
const checksums = [
  `${'a'.repeat(64)}  mcp-gate-0.1.0.tgz`,
  `${arm}  mcp-gate-0.1.0-darwin-arm64.tar.gz`,
  `${intel}  mcp-gate-0.1.0-darwin-x64.tar.gz`,
  `${'d'.repeat(64)}  mcp-gate-0.1.0-win32-x64.tar.gz`,
  '',
].join('\n');

test('Homebrew formula installs both macOS archives and does not register a service', () => {
  const formula = renderHomebrewFormula(template, checksums, '0.1.0');
  assert.match(formula, /version "0\.1\.0"/);
  assert.match(formula, new RegExp(`sha256 "${arm}"`));
  assert.match(formula, new RegExp(`sha256 "${intel}"`));
  assert.match(formula, /on_macos do/);
  assert.match(formula, /on_arm do/);
  assert.match(formula, /on_intel do/);
  assert.match(formula, /depends_on :macos/);
  assert.match(formula, /bin\.install "bin\/mcp-gate"/);
  assert.match(formula, /mcp-gate-#\{version\}-darwin-arm64\.tar\.gz/);
  assert.match(formula, /mcp-gate-#\{version\}-darwin-x64\.tar\.gz/);
  assert.match(formula, /Re-run `mcp-gate setup` after upgrading/);
  assert.match(formula, /mcp-gate --version/);
  assert.doesNotMatch(formula, /service do/);
  assert.doesNotMatch(formula, /aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa/);
  assert.doesNotMatch(formula, /\{\{/);
});

test('Homebrew rendering rejects a checksum file that cannot fill the formula', () => {
  assert.throws(() => renderHomebrewFormula(template, checksums, '0.1.0-compat-fixture'), /stable release/);
  assert.throws(() => renderHomebrewFormula(template, `${arm}  other.tar.gz\n`, '0.1.0'), /macOS native archive/);
  assert.throws(() => renderHomebrewFormula(template, `NOT-A-SUM  mcp-gate-0.1.0-darwin-arm64.tar.gz\n`, '0.1.0'), /Malformed/);
  const duplicated = `${checksums}${arm}  mcp-gate-0.1.0-darwin-arm64.tar.gz\n`;
  assert.throws(() => renderHomebrewFormula(template, duplicated, '0.1.0'), /Duplicate SHA256SUMS entry/);
  const withService = template.replace('def install', 'service do\n  end\n\n  def install');
  assert.throws(() => renderHomebrewFormula(withService, checksums, '0.1.0'), /must not install a service/);
});
