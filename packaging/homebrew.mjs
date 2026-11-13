// Render the personal-tap formula from the native archives listed in SHA256SUMS.
import { readFileSync, writeFileSync } from 'node:fs';
import { pathToFileURL } from 'node:url';

const requirements = [
  ['depends_on :macos', 'macOS requirement'],
  ['on_arm do', 'Apple silicon archive'],
  ['on_intel do', 'Intel archive'],
  ['mcp-gate setup', 'setup caveat'],
  ['mcp-gate --version', 'version test'],
];

export function renderHomebrewFormula(template, checksums, version) {
  if (!/^\d+\.\d+\.\d+$/.test(version)) throw Error('Homebrew formula version must be a stable release');
  const digests = new Map();
  for (const line of checksums.split('\n')) {
    if (line === '') continue;
    const match = /^([0-9a-f]{64})  (\S+)$/.exec(line);
    if (!match) throw Error('Malformed SHA256SUMS line');
    if (digests.has(match[2])) throw Error(`Duplicate SHA256SUMS entry: ${match[2]}`);
    digests.set(match[2], match[1]);
  }
  const arm64 = digests.get(`mcp-gate-${version}-darwin-arm64.tar.gz`);
  const intel = digests.get(`mcp-gate-${version}-darwin-x64.tar.gz`);
  if (!arm64 || !intel) throw Error('SHA256SUMS is missing a macOS native archive');
  const rendered = template
    .replaceAll('{{version}}', version)
    .replaceAll('{{sha256_darwin_arm64}}', arm64)
    .replaceAll('{{sha256_darwin_x64}}', intel);
  if (rendered.includes('{{')) throw Error('Homebrew template has an unreplaced placeholder');
  for (const [needle, label] of requirements) {
    if (!rendered.includes(needle)) throw Error(`Homebrew formula is missing its ${label}`);
  }
  if (/\bservice do\b/.test(rendered)) throw Error('Homebrew formula must not install a service');
  return rendered;
}

function main() {
  const [checksumPath, version, output] = process.argv.slice(2);
  if (!checksumPath || !version || !output) {
    throw Error('Usage: node packaging/homebrew.mjs SHA256SUMS VERSION OUTPUT');
  }
  const template = readFileSync(new URL('./homebrew/mcp-gate.rb.tmpl', import.meta.url), 'utf8');
  const formula = renderHomebrewFormula(template, readFileSync(checksumPath, 'utf8'), version);
  writeFileSync(output, formula);
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  main();
}
