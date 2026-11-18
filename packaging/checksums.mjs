// Confirm every npm package and native archive, including one shared launcher.
// SHA256SUMS lists only the native archives attached to the GitHub Release.
// npm tarballs are published to the registry and checked by their registry integrity.
import { readdirSync, readFileSync, writeFileSync } from 'node:fs';
import { createHash } from 'node:crypto';
import { join, basename } from 'node:path';
import { nativeTargets } from './manifest.mjs';

const root = process.argv[2] ?? 'artifacts';
const rows = [];
for (const directory of readdirSync(root, { withFileTypes: true }).filter(entry => entry.isDirectory())) {
  const names = readdirSync(join(root, directory.name))
    .filter(name => name.endsWith('.tgz') || name.endsWith('.tar.gz') || name.endsWith('.zip'));
  for (const name of names) {
    const bytes = readFileSync(join(root, directory.name, name));
    const digest = createHash('sha256').update(bytes).digest('hex');
    const previous = rows.find(row => row.name === basename(name));
    if (previous && previous.digest !== digest) throw Error(`Duplicate archive differs: ${name}`);
    if (!previous) rows.push({ digest, name: basename(name) });
  }
}
const packages = rows.filter(row => row.name.endsWith('.tgz'));
const archives = rows.filter(row => row.name.endsWith('.tar.gz'));
const expectedArchives = nativeTargets.length;
const expectedPackages = expectedArchives + 1;
if (packages.length !== expectedPackages || archives.length !== expectedArchives
  || rows.length !== expectedPackages + expectedArchives) {
  const found = `${rows.length} files (${packages.length} npm packages and ${archives.length} native archives)`;
  throw Error(`Expected ${expectedPackages} npm packages and ${expectedArchives} native archives; found ${found}`);
}
const lines = archives
  .sort((left, right) => left.name.localeCompare(right.name))
  .map(row => `${row.digest}  ${row.name}\n`);
writeFileSync(join(root, 'SHA256SUMS'), lines.join(''));
