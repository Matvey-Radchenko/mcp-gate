import launcher from '../npm/mcp-gate/bin/mcp-gate.cjs';

// Shared identity rules for the meta package and generated platform packages.
export const nativeTargets = [
  'darwin-arm64', 'darwin-x64', 'win32-x64', 'win32-arm64',
  'linux-x64', 'linux-arm64', 'linux-x64-musl', 'linux-arm64-musl',
];
export const npmScope = '@matvei-radchenko';
export const launcherPackage = `${npmScope}/mcp-gate`;

export function nativeBuildTarget() {
  const preferred = launcher.preferredTarget();
  const target = process.env.MCP_GATE_NATIVE_TARGET ?? preferred;
  // CI compiles and tests the static musl build on a glibc host of the same architecture.
  // Its npm archive is smoke-tested only on musl (Alpine), because npm installs it only there.
  if (!nativeTargets.includes(target) || (target !== preferred && target !== `${preferred}-musl`)) {
    throw Error(`Unsupported native target ${target} on ${preferred}`);
  }
  return target;
}

// An explicit Cargo target confines static CRT flags to target crates and keeps
// host proc-macro libraries on their normal runtime.
export function staticRustTarget(target) {
  const [os, cpu, libc] = target.split('-');
  const arch = { x64: 'x86_64', arm64: 'aarch64' }[cpu];
  if (os === 'win32') return `${arch}-pc-windows-msvc`;
  if (libc === 'musl') return `${arch}-unknown-linux-musl`;
}

export function platformPackageName(target) {
  if (!nativeTargets.includes(target)) throw Error(`Unsupported native target: ${target}`);
  return `${npmScope}/mcp-gate-bin-${target}`;
}

// npm pack names a scoped package by dropping @ and turning / into -.
export function npmTarball(name, version) {
  return `${name.replace(/^@/, '').replaceAll('/', '-')}-${version}.tgz`;
}

// Registry URLs keep @ and encode the scope slash.
export function registryName(name) {
  return name.replaceAll('/', '%2f');
}

export function platformPackage(manifest, target) {
  // os, cpu and (on Linux) libc select this binary. The launcher owns the Node engine requirement.
  const [os, cpu, libc] = target.split('-');
  return {
    name: platformPackageName(target),
    version: manifest.version,
    publishConfig: { access: 'public' },
    description: `Native mcp-gate for ${target}`,
    keywords: manifest.keywords,
    homepage: manifest.homepage,
    bugs: manifest.bugs,
    license: manifest.license,
    author: manifest.author,
    repository: manifest.repository,
    os: [os],
    cpu: [cpu],
    // npm installs exactly one Linux package per host, so glibc hosts never download musl.
    ...(os === 'linux' ? { libc: [libc === 'musl' ? 'musl' : 'glibc'] } : {}),
    files: ['bin/', 'LICENSE', 'README.md'],
  };
}

export function assertReleaseManifest(manifest, cargoToml, launcherSource) {
  const cargoVersion = cargoToml.match(/^version = "([^"]+)"/m)?.[1];
  if (cargoVersion !== manifest.version) throw Error('Cargo/npm version mismatch');
  if (manifest.name !== launcherPackage) throw Error('Launcher package is not the scoped name');
  if (manifest.publishConfig?.access !== 'public') {
    throw Error('Scoped packages must set publishConfig.access to public');
  }
  for (const target of nativeTargets) {
    const name = platformPackageName(target);
    if (manifest.optionalDependencies?.[name] !== manifest.version) {
      throw Error(`${name} version does not match the launcher`);
    }
  }
  for (const field of ['license', 'author', 'homepage', 'bugs', 'keywords', 'repository', 'engines']) {
    if (manifest[field] == null) throw Error(`npm manifest is missing ${field}`);
  }
  if (!Array.isArray(manifest.keywords) || manifest.keywords.length === 0) {
    throw Error('npm manifest needs keywords');
  }
  if (typeof manifest.engines?.node !== 'string' || manifest.engines.node === '') {
    throw Error('npm manifest is missing engines.node');
  }
  if (!launcherSource.includes('package.json') || !/\bversion\b/.test(launcherSource)
    || /\b\d+\.\d+\.\d+\b/.test(launcherSource)) {
    throw Error('Launcher must read its version from package.json');
  }
  if (!launcherSource.includes('require.resolve(') || !launcherSource.includes(`${npmScope}/mcp-gate-bin-`)) {
    throw Error('Launcher must resolve the scoped platform package');
  }
  if (!launcherSource.includes('optionalDependencies')) {
    throw Error('Launcher must read supported targets from optionalDependencies');
  }
}
