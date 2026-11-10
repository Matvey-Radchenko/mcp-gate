import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { cpSync, existsSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { createRequire } from 'node:module';
import { tmpdir } from 'node:os';
import { dirname, join } from 'node:path';
import test from 'node:test';
import { assertReleaseManifest, platformPackage } from './manifest.mjs';

const require = createRequire(import.meta.url);
const manifest = JSON.parse(readFileSync('npm/mcp-gate/package.json', 'utf8'));
const cargoToml = readFileSync('Cargo.toml', 'utf8');
const launcherSource = readFileSync('npm/mcp-gate/bin/mcp-gate.cjs', 'utf8');
const { packageVersion, unsupportedMessage } = require('../npm/mcp-gate/bin/mcp-gate.cjs');

function npmCli() {
  const fromEnv = process.env.npm_execpath;
  if (fromEnv && existsSync(fromEnv)) return fromEnv;
  const root = dirname(process.execPath);
  const candidates = [
    join(root, 'node_modules/npm/bin/npm-cli.js'),
    join(root, '../lib/node_modules/npm/bin/npm-cli.js'),
  ];
  return candidates.find(path => existsSync(path));
}

function runNpm(args, options = {}) {
  const cli = npmCli();
  // Windows registers npm as npm.cmd. spawn cannot execute that without a shell,
  // so prefer the CLI script next to node and only then fall back to the command.
  const result = cli
    ? spawnSync(process.execPath, [cli, ...args], { encoding: 'utf8', windowsHide: true, ...options })
    : spawnSync(process.platform === 'win32' ? 'npm.cmd' : 'npm', args, {
      encoding: 'utf8',
      windowsHide: true,
      shell: process.platform === 'win32',
      ...options,
    });
  assert.equal(result.error, undefined, result.error?.message);
  return result;
}

test('launcher version comes from package.json and matches Cargo and optional packages', () => {
  assert.equal(manifest.author, 'Matvey Radchenko');
  assert.equal(manifest.homepage, 'https://github.com/Matvey-Radchenko/mcp-gate');
  assert.deepEqual(manifest.bugs, { url: 'https://github.com/Matvey-Radchenko/mcp-gate/issues' });
  assert.deepEqual(manifest.keywords, ['mcp', 'model-context-protocol', 'gateway', 'stdio', 'agent']);
  assert.deepEqual(manifest.engines, { node: '>=18' });
  assert.deepEqual(manifest.os, ['darwin', 'win32']);
  assert.equal(manifest.license, 'MIT');
  assert.deepEqual(manifest.files, ['bin/', 'README.md', 'LICENSE']);
  assert.equal(manifest.scripts.prepack, 'node scripts/prepack.cjs');
  assertReleaseManifest(manifest, cargoToml, launcherSource);
  assert.equal(packageVersion(), manifest.version);
  const target = 'linux-x64';
  assert.equal(
    unsupportedMessage(packageVersion(), target),
    `mcp-gate ${manifest.version} does not support ${target}. Use macOS ARM64/Intel or Windows x64.`,
  );
  assert.throws(
    () => assertReleaseManifest(manifest, 'version = "9.9.9"\n', launcherSource),
    /Cargo\/npm version mismatch/,
  );
  const drifted = structuredClone(manifest);
  drifted.optionalDependencies['mcp-gate-bin-win32-x64'] = '0.1.1';
  assert.throws(() => assertReleaseManifest(drifted, cargoToml, launcherSource), /win32-x64/);
  assert.throws(
    () => assertReleaseManifest(manifest, cargoToml, "console.error('mcp-gate 9.9.9')"),
    /package\.json/,
  );
  assert.throws(
    () => assertReleaseManifest(manifest, cargoToml, "readFileSync('package.json')"),
    /darwin-arm64/,
  );
});

test('platform packages copy metadata and keep native os, cpu, and files', () => {
  const expected = {
    'darwin-arm64': ['darwin', 'arm64'],
    'darwin-x64': ['darwin', 'x64'],
    'win32-x64': ['win32', 'x64'],
  };
  for (const [target, [os, cpu]] of Object.entries(expected)) {
    const pkg = platformPackage(manifest, target);
    assert.equal(pkg.name, `mcp-gate-bin-${target}`);
    assert.equal(pkg.version, manifest.version);
    assert.equal(pkg.author, manifest.author);
    assert.equal(pkg.homepage, manifest.homepage);
    assert.deepEqual(pkg.bugs, manifest.bugs);
    assert.deepEqual(pkg.keywords, manifest.keywords);
    assert.deepEqual(pkg.license, manifest.license);
    assert.deepEqual(pkg.repository, manifest.repository);
    assert.equal(pkg.engines, undefined);
    assert.deepEqual(pkg.os, [os]);
    assert.deepEqual(pkg.cpu, [cpu]);
    assert.deepEqual(pkg.files, ['bin/', 'LICENSE', 'README.md']);
    assert.equal(pkg.bin, undefined);
    assert.equal(pkg.optionalDependencies, undefined);
  }
  assert.throws(() => platformPackage(manifest, 'linux-x64'), /Unsupported native target/);
});

test('source tree refuses npm pack until README and LICENSE are staged', () => {
  const blocked = spawnSync(process.execPath, ['npm/mcp-gate/scripts/prepack.cjs'], { encoding: 'utf8' });
  assert.equal(blocked.status, 1);
  assert.match(blocked.stderr, /Refusing to pack mcp-gate without README\.md and LICENSE/);

  const packed = runNpm(['pack', '--dry-run', '--json', './npm/mcp-gate']);
  assert.notEqual(packed.status, 0);
  assert.match(`${packed.stderr}\n${packed.stdout}`, /Refusing to pack mcp-gate without README\.md and LICENSE/);

  const root = mkdtempSync(join(tmpdir(), 'mcp-gate pack '));
  try {
    const staged = join(root, 'mcp-gate');
    cpSync('npm/mcp-gate', staged, { recursive: true });
    writeFileSync(join(staged, 'README.md'), 'mcp-gate\n');
    writeFileSync(join(staged, 'LICENSE'), 'MIT\n');
    assert.equal(spawnSync(process.execPath, [join(staged, 'scripts/prepack.cjs')], { encoding: 'utf8' }).status, 0);
    for (const ignore of [false, true]) {
      const args = ['pack', '--dry-run', '--json', './mcp-gate'];
      if (ignore) args.push('--ignore-scripts');
      const result = runNpm(args, { cwd: root });
      assert.equal(result.status, 0, result.stderr);
      const names = JSON.parse(result.stdout)[0].files.map(file => file.path);
      assert.ok(names.includes('README.md'));
      assert.ok(names.includes('LICENSE'));
      assert.ok(names.includes('bin/mcp-gate.cjs'));
      assert.equal(names.includes('scripts/prepack.cjs'), false);
    }
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});

const supported = ['darwin-arm64', 'darwin-x64', 'win32-x64'];
test('unsupported platforms exit with the package.json version', {
  skip: supported.includes(`${process.platform}-${process.arch}`),
}, () => {
  const result = spawnSync(process.execPath, ['npm/mcp-gate/bin/mcp-gate.cjs'], { encoding: 'utf8' });
  assert.equal(result.status, 1);
  const version = manifest.version.replaceAll('.', '\\.');
  assert.match(result.stderr, new RegExp(`mcp-gate ${version} does not support ${process.platform}-${process.arch}`));
});
