#!/usr/bin/env node
'use strict';
const { spawn } = require('node:child_process');
const { readFileSync } = require('node:fs');
const { join } = require('node:path');

const supported = ['darwin-arm64', 'darwin-x64', 'win32-x64'];

function packageVersion() {
  const { version } = JSON.parse(readFileSync(join(__dirname, '..', 'package.json'), 'utf8'));
  if (typeof version !== 'string' || version === '') {
    console.error('mcp-gate package.json is missing a version.');
    process.exit(1);
  }
  return version;
}

function unsupportedMessage(version, target) {
  return `mcp-gate ${version} does not support ${target}. Use macOS ARM64/Intel or Windows x64.`;
}

function main() {
  const version = packageVersion();
  const target = `${process.platform}-${process.arch}`;
  if (!supported.includes(target)) {
    console.error(unsupportedMessage(version, target));
    process.exit(1);
  }
  let binary;
  try {
    const suffix = process.platform === 'win32' ? '.exe' : '';
    binary = require.resolve(`@matvei-radchenko/mcp-gate-bin-${target}/bin/mcp-gate${suffix}`);
  } catch {
    console.error('The native mcp-gate package is missing. Reinstall with optional dependencies enabled, or download the matching GitHub Release binary.');
    process.exit(1);
  }
  const child = spawn(binary, process.argv.slice(2), { stdio: 'inherit', windowsHide: true });
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

module.exports = { packageVersion, unsupportedMessage };
