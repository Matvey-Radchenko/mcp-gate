'use strict';
// npm pack runs package scripts from the package root. README.md and LICENSE
// are not in this source directory; packaging/launcher.mjs copies them into
// dist/mcp-gate before CI packs that tree with --ignore-scripts.
const { existsSync } = require('node:fs');
const { join } = require('node:path');

const root = join(__dirname, '..');
const missing = ['README.md', 'LICENSE'].filter(name => !existsSync(join(root, name)));
if (missing.length === 0) process.exit(0);
console.error(`Refusing to pack mcp-gate without ${missing.join(' and ')}.`);
console.error('Stage the package with `node packaging/launcher.mjs`, then pack dist/mcp-gate.');
process.exit(1);
