#!/usr/bin/env node
'use strict';
const { spawn } = require('node:child_process');
const target = `${process.platform}-${process.arch}`;
if (!['darwin-arm64', 'darwin-x64', 'win32-x64'].includes(target)) {
  console.error(`mcp-gate 0.1.0 does not support ${target}. Use macOS ARM64/Intel or Windows x64.`);
  process.exit(1);
}
let binary;
try {
  binary = require.resolve(`mcp-gate-${target}/bin/mcp-gate${process.platform === 'win32' ? '.exe' : ''}`);
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
