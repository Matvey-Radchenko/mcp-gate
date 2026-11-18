import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { closeSync, cpSync, existsSync, mkdtempSync, openSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { dirname, join } from 'node:path';
import test from 'node:test';
import { assertReleaseManifest, launcherPackage, nativeTargets, npmTarball, platformPackage, platformPackageName } from './manifest.mjs';

const manifest = JSON.parse(readFileSync('npm/mcp-gate/package.json', 'utf8'));
const cargoToml = readFileSync('Cargo.toml', 'utf8');
const launcherSource = readFileSync('npm/mcp-gate/bin/mcp-gate.cjs', 'utf8');

function captureCommand(command, args, options = {}) {
  // npm and the launcher write diagnostics even when exiting immediately.
  // Regular files keep those writes synchronous on every host.
  const root = mkdtempSync(join(tmpdir(), 'mcp-gate output '));
  const stdout = join(root, 'stdout');
  const stderr = join(root, 'stderr');
  const files = [];
  try {
    files.push(openSync(stdout, 'w'));
    files.push(openSync(stderr, 'w'));
    const result = spawnSync(command, args, { windowsHide: true, ...options, stdio: ['ignore', ...files] });
    assert.equal(result.error, undefined, result.error?.message);
    return { status: result.status, stdout: readFileSync(stdout, 'utf8'), stderr: readFileSync(stderr, 'utf8') };
  } finally {
    for (const file of files) closeSync(file);
    rmSync(root, { recursive: true, force: true });
  }
}

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
  return cli
    ? captureCommand(process.execPath, [cli, ...args], options)
    : captureCommand(process.platform === 'win32' ? 'npm.cmd' : 'npm', args, {
      shell: process.platform === 'win32',
      ...options,
    });
}

test('launcher version comes from package.json and matches Cargo and optional packages', () => {
  assert.equal(manifest.name, launcherPackage);
  assert.deepEqual(manifest.publishConfig, { access: 'public' });
  assert.equal(npmTarball(launcherPackage, manifest.version), 'matvei-radchenko-mcp-gate-0.1.2.tgz');
  assert.equal(manifest.author, 'Matvey Radchenko');
  assert.equal(manifest.homepage, 'https://github.com/Matvey-Radchenko/mcp-gate');
  assert.deepEqual(manifest.bugs, { url: 'https://github.com/Matvey-Radchenko/mcp-gate/issues' });
  assert.deepEqual(manifest.keywords, ['mcp', 'model-context-protocol', 'gateway', 'stdio', 'agent']);
  assert.deepEqual(manifest.engines, { node: '>=18' });
  assert.deepEqual(manifest.os, ['darwin', 'win32', 'linux']);
  assert.equal(manifest.license, 'MIT');
  assert.deepEqual(manifest.files, ['bin/', 'README.md', 'LICENSE']);
  assert.equal(manifest.scripts.prepack, 'node scripts/prepack.cjs');
  assertReleaseManifest(manifest, cargoToml, launcherSource);
  assert.deepEqual(Object.keys(manifest.optionalDependencies).sort(), nativeTargets.map(platformPackageName).sort());
  assert.throws(
    () => assertReleaseManifest(manifest, 'version = "9.9.9"\n', launcherSource),
    /Cargo\/npm version mismatch/,
  );
  const drifted = structuredClone(manifest);
  drifted.optionalDependencies[platformPackageName('win32-x64')] = '9.9.9';
  assert.throws(() => assertReleaseManifest(drifted, cargoToml, launcherSource), /win32-x64/);
  assert.throws(
    () => assertReleaseManifest(manifest, cargoToml, `${launcherSource}\nconsole.error('mcp-gate 9.9.9')`),
    /package\.json/,
  );
  assert.throws(
    () => assertReleaseManifest(manifest, cargoToml, "const { version } = require('../package.json');"),
    /scoped platform package/,
  );
  assert.throws(
    () => assertReleaseManifest(
      manifest,
      cargoToml,
      "const { version } = require('../package.json'); require.resolve('@matvei-radchenko/mcp-gate-bin-' + target);",
    ),
    /read supported targets from optionalDependencies/,
  );
});

test('platform packages copy metadata and keep native os, cpu, and files', () => {
  const expected = {
    'darwin-arm64': ['darwin', 'arm64'],
    'darwin-x64': ['darwin', 'x64'],
    'win32-x64': ['win32', 'x64'],
    'linux-x64': ['linux', 'x64', ['glibc']],
    'linux-arm64': ['linux', 'arm64', ['glibc']],
  };
  for (const [target, [os, cpu, libc]] of Object.entries(expected)) {
    const pkg = platformPackage(manifest, target);
    assert.equal(pkg.name, platformPackageName(target));
    assert.deepEqual(pkg.publishConfig, { access: 'public' });
    assert.equal(npmTarball(pkg.name, pkg.version), `matvei-radchenko-mcp-gate-bin-${target}-0.1.2.tgz`);
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
    assert.deepEqual(pkg.libc, libc);
    assert.equal(Object.hasOwn(pkg, 'libc'), os === 'linux');
    assert.deepEqual(pkg.files, ['bin/', 'LICENSE', 'README.md']);
    assert.equal(pkg.bin, undefined);
    assert.equal(pkg.optionalDependencies, undefined);
  }
  assert.throws(() => platformPackage(manifest, 'linux-ia32'), /Unsupported native target/);
});

test('source tree refuses npm pack until README and LICENSE are staged', () => {
  const blocked = captureCommand(process.execPath, ['npm/mcp-gate/scripts/prepack.cjs']);
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
    assert.equal(captureCommand(process.execPath, [join(staged, 'scripts/prepack.cjs')]).status, 0);
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

test('unsupported platforms exit with the package.json version', {
  skip: nativeTargets.includes(`${process.platform}-${process.arch}`),
}, () => {
  const result = captureCommand(process.execPath, ['npm/mcp-gate/bin/mcp-gate.cjs']);
  assert.equal(result.status, 1);
  const version = manifest.version.replaceAll('.', '\\.');
  assert.match(result.stderr, new RegExp(`mcp-gate ${version} does not support ${process.platform}-${process.arch}`));
});

test('Linux launcher distinguishes unsupported targets from missing glibc packages', t => {
  const root = mkdtempSync(join(tmpdir(), 'mcp-gate launcher '));
  t.after(() => rmSync(root, { recursive: true, force: true }));
  cpSync('npm/mcp-gate/bin', join(root, 'bin'), { recursive: true });
  const version = '9.9.9';
  writeFileSync(join(root, 'package.json'), JSON.stringify({ ...manifest, version }));
  const preload = join(root, 'target.cjs');
  for (const arch of ['ia32', 'x64', 'arm64']) {
    writeFileSync(preload, `
      Object.defineProperty(process, 'platform', { value: 'linux' });
      Object.defineProperty(process, 'arch', { value: '${arch}' });
    `);
    const result = captureCommand(process.execPath, ['--require', preload, join(root, 'bin/mcp-gate.cjs')]);
    assert.equal(result.status, 1, result.stderr);
    assert.match(result.stderr, /Linux requires glibc \(2\.35\+\)/);
    if (arch === 'ia32') {
      assert.ok(result.stderr.includes(`mcp-gate ${version} does not support linux-ia32`));
      assert.ok(result.stderr.includes(`Supported targets: ${nativeTargets.join(', ')}`));
    } else {
      assert.ok(result.stderr.includes(`The native mcp-gate package is missing for linux-${arch}`));
      assert.match(result.stderr, /optional dependencies enabled.*matching GitHub Release binary/);
    }
  }
});
