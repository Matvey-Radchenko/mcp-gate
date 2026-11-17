#!/usr/bin/env node
'use strict';
const { spawn } = require('node:child_process');
const { toNamespacedPath } = require('node:path');

const packagePrefix = '@matvei-radchenko/mcp-gate-bin-';

function main() {
  const { version, optionalDependencies = {} } = require('../package.json');
  if (typeof version !== 'string' || version === '') {
    console.error('mcp-gate package.json is missing a version.');
    process.exit(1);
  }
  const target = `${process.platform}-${process.arch}`;
  const nativePackage = `${packagePrefix}${target}`;
  if (!Object.hasOwn(optionalDependencies, nativePackage)) {
    const supported = Object.keys(optionalDependencies)
      .filter(name => name.startsWith(packagePrefix))
      .map(name => name.slice(packagePrefix.length));
    console.error(`mcp-gate ${version} does not support ${target}. Supported targets: ${supported.join(', ')}. Linux requires glibc (2.35+).`);
    process.exit(1);
  }
  let binary;
  try {
    const suffix = process.platform === 'win32' ? '.exe' : '';
    binary = require.resolve(`${nativePackage}/bin/mcp-gate${suffix}`);
  } catch {
    const linux = process.platform === 'linux' ? ' Linux requires glibc (2.35+).' : '';
    console.error(`The native mcp-gate package is missing for ${target}.${linux} Reinstall with optional dependencies enabled, or download the matching GitHub Release binary.`);
    process.exit(1);
  }
  // pnpm caches can exceed MAX_PATH; Windows needs an extended-length executable path
  const child = spawn(toNamespacedPath(binary), process.argv.slice(2), { stdio: 'inherit', windowsHide: true });
  child.once('error', () => {
    console.error('Cannot start the native mcp-gate binary. Check the installation and file permissions.');
    process.exitCode = 1;
  });
  for (const signal of ['SIGINT', 'SIGTERM']) {
    process.on(signal, () => child.kill(signal));
  }
  child.once('exit', (code, signal) => {
    process.exitCode = code ?? (signal === 'SIGINT' ? 130 : 1);
  });
}

if (require.main === module) main();
