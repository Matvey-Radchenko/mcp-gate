#!/usr/bin/env node
'use strict';
const { spawn } = require('node:child_process');
const { existsSync } = require('node:fs');
const { toNamespacedPath } = require('node:path');

const packagePrefix = '@matvei-radchenko/mcp-gate-bin-';

function preferredTarget() {
  const target = `${process.platform}-${process.arch}`;
  if (process.platform !== 'linux') return target;
  const header = process.report?.getReport?.()?.header;
  if (header) return header.glibcVersionRuntime ? target : `${target}-musl`;
  const loaderArch = { x64: 'x86_64', arm64: 'aarch64' }[process.arch];
  return loaderArch && existsSync(`/lib/ld-musl-${loaderArch}.so.1`) ? `${target}-musl` : target;
}

function resolveBinary(nativePackage) {
  const suffix = process.platform === 'win32' ? '.exe' : '';
  try {
    return require.resolve(`${nativePackage}/bin/mcp-gate${suffix}`);
  } catch (error) {
    if (error.code !== 'MODULE_NOT_FOUND') throw error;
  }
}

function main() {
  const { version, optionalDependencies = {} } = require('../package.json');
  if (typeof version !== 'string' || version === '') {
    console.error('mcp-gate package.json is missing a version.');
    process.exit(1);
  }
  const target = preferredTarget();
  // npm installs only the package whose os, cpu and libc match this host.
  const nativePackage = `${packagePrefix}${target}`;
  if (!Object.hasOwn(optionalDependencies, nativePackage)) {
    const supported = Object.keys(optionalDependencies)
      .filter(name => name.startsWith(packagePrefix))
      .map(name => name.slice(packagePrefix.length));
    console.error(`mcp-gate ${version} does not support ${target}. Supported targets: ${supported.join(', ')}.`);
    process.exit(1);
  }
  const binary = resolveBinary(nativePackage);
  if (!binary) {
    console.error(`The native mcp-gate package is missing for ${target}. Reinstall with optional dependencies enabled, or download the matching GitHub Release binary.`);
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

module.exports = { preferredTarget };
if (require.main === module) main();
