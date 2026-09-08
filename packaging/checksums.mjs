import { readdirSync, readFileSync, writeFileSync } from 'node:fs';
import { createHash } from 'node:crypto';
import { join, basename } from 'node:path';
const root=process.argv[2] ?? 'artifacts';
const rows=[];
for (const directory of readdirSync(root,{withFileTypes:true}).filter(d=>d.isDirectory())) {
  for (const name of readdirSync(join(root,directory.name)).filter(n=>n.endsWith('.tgz')||n.endsWith('.tar.gz')||n.endsWith('.zip'))) {
    const digest=createHash('sha256').update(readFileSync(join(root,directory.name,name))).digest('hex');
    const previous=rows.find(row=>row.name===basename(name));
    if (previous && previous.digest!==digest) throw Error(`Duplicate archive differs: ${name}`);
    if (!previous) rows.push({digest,name:basename(name)});
  }
}
if (rows.length!==7) throw Error(`Expected four npm packages and three native archives; found ${rows.length}`);
writeFileSync(join(root,'SHA256SUMS'),rows.sort((a,b)=>a.name.localeCompare(b.name)).map(r=>`${r.digest}  ${r.name}\n`).join(''));
