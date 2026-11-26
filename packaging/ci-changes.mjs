// Decide which CI jobs a pull request needs from the files it changes.
// Unknown paths run everything: a missed check costs more than a slow PR.
import { appendFileSync, readFileSync } from 'node:fs';
import { pathToFileURL } from 'node:url';

// Ordered: the first matching rule wins for a file. `scope` is what the file
// can affect; each scope also runs every cheaper scope's jobs.
export const RULES = [
  // Rust sources, test fixtures, compiled-in recipes, toolchain and workflows
  // reach the binary or the Docker tests, so they run every job.
  { scope: 'full', why: 'workflows', match: p => p.startsWith('.github/workflows/') },
  { scope: 'full', why: 'Rust sources and tests', match: p => /^(src|tests)\//.test(p) || p === 'build.rs' },
  { scope: 'full', why: 'Cargo manifests', match: p => ['Cargo.toml', 'Cargo.lock'].includes(p) || p.startsWith('.cargo/') },
  { scope: 'full', why: 'recipes compiled into the binary', match: p => p.startsWith('recipes/') },
  { scope: 'full', why: 'checkout line endings', match: p => p === '.gitattributes' },
  { scope: 'full', why: 'Windows Docker acceptance script', match: p => p === 'packaging/test-docker-windows.ps1' },
  // Lints, packaging and npm launcher code run only inside the native matrix.
  { scope: 'native', why: 'lint and size budgets', match: p => ['quality.toml', 'clippy.toml', 'rustfmt.toml'].includes(p) || p.startsWith('xtask/') },
  { scope: 'native', why: 'packaging scripts', match: p => p.startsWith('packaging/') },
  { scope: 'native', why: 'npm launcher package', match: p => p.startsWith('npm/mcp-gate/') },
  { scope: 'native', why: 'browser MCP fixtures', match: p => p.startsWith('runtime/') },
  // Copied into the npm packages; the launcher job checks they are packed.
  { scope: 'launcher', why: 'npm package README and license', match: p => p === 'npm/README.md' || p === 'LICENSE' },
  // Release evidence is read only by the release gate and its script tests.
  { scope: 'scripts', why: 'release evidence', match: p => p.startsWith('release/') && p.endsWith('.json') },
  // Read by people only. Nothing builds, packs or tests these files.
  {
    scope: 'docs',
    why: 'documentation',
    match: p => p.startsWith('docs/') || p.startsWith('skills/') || p.startsWith('.github/ISSUE_TEMPLATE/')
      || ['.github/PULL_REQUEST_TEMPLATE.md', 'release/notes.md', 'release/source-audit.md', '.editorconfig', '.gitignore'].includes(p)
      || (!p.includes('/') && p.endsWith('.md')),
  },
];

const ORDER = ['docs', 'scripts', 'launcher', 'native', 'full'];

export function classify(files) {
  const paths = files.map(f => f.trim()).filter(Boolean);
  const matched = paths.map(path => {
    const rule = RULES.find(r => r.match(path));
    return { path, scope: rule?.scope ?? 'full', why: rule?.why ?? 'not classified, so run everything' };
  });
  // An empty diff is unexpected; do not let it skip anything.
  const scope = paths.length === 0
    ? 'full'
    : matched.reduce((top, m) => (ORDER.indexOf(m.scope) > ORDER.indexOf(top) ? m.scope : top), 'docs');
  const at = level => ORDER.indexOf(scope) >= ORDER.indexOf(level);
  return { scope, launcher: at('launcher'), native: at('native'), docker: at('full'), files: matched };
}

if (import.meta.url === pathToFileURL(process.argv[1]).href) {
  const result = process.argv[2] === '--all'
    ? { ...classify([]), files: [] }
    : classify(readFileSync(0, 'utf8').split('\n'));
  const outputs = { scope: result.scope, launcher: result.launcher, native: result.native, docker: result.docker };
  const lines = Object.entries(outputs).map(([k, v]) => `${k}=${v}`);
  if (process.env.GITHUB_OUTPUT) appendFileSync(process.env.GITHUB_OUTPUT, `${lines.join('\n')}\n`);
  const table = result.files.map(f => `| \`${f.path}\` | ${f.scope} | ${f.why} |`);
  const summary = [
    `### CI scope: ${result.scope}`,
    '',
    `npm launcher: ${result.launcher}, native matrix: ${result.native}, Docker: ${result.docker}`,
    ...(table.length ? ['', '| File | Scope | Reason |', '| --- | --- | --- |', ...table] : ['', 'Not a pull request: every job runs.']),
  ].join('\n');
  if (process.env.GITHUB_STEP_SUMMARY) appendFileSync(process.env.GITHUB_STEP_SUMMARY, `${summary}\n`);
  console.log(summary);
}
