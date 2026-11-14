// Shared identity rules for the meta package and generated platform packages.
export const nativeTargets = ['darwin-arm64', 'darwin-x64', 'win32-x64'];
export const npmScope = '@matvei-radchenko';
export const launcherPackage = `${npmScope}/mcp-gate`;

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
  // os and cpu select this binary. The Node engine requirement stays on the launcher.
  const [os, cpu] = target.split('-');
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
  if (!launcherSource.includes('package.json') || /(['"`])\d+\.\d+\.\d+\1/.test(launcherSource)) {
    throw Error('Launcher must read its version from package.json');
  }
  if (!launcherSource.includes(`${npmScope}/mcp-gate-bin-`)) {
    throw Error('Launcher must resolve the scoped platform package');
  }
  for (const target of nativeTargets) {
    if (!launcherSource.includes(target)) throw Error(`Launcher is missing support for ${target}`);
  }
}
