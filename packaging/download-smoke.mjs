import { mkdtempSync,writeFileSync,rmSync,mkdirSync } from 'node:fs';
import { join } from 'node:path';
import { tmpdir } from 'node:os';
import { createHash } from 'node:crypto';
import { spawnSync } from 'node:child_process';
import assert from 'node:assert/strict';
const version=process.argv[2];assert.match(version,/^\d+\.\d+\.\d+$/);
const target=`${process.platform}-${process.arch}`;
assert(['darwin-arm64','darwin-x64','win32-x64'].includes(target));
const filename=`mcp-gate-${version}-${target}.tar.gz`;
const base=`https://github.com/Matvey-Radchenko/mcp-gate/releases/download/v${version}`;
async function download(name) {
  const response=await fetch(`${base}/${name}`);assert(response.ok,`Download failed: ${name} ${response.status}`);
  return Buffer.from(await response.arrayBuffer());
}
const directory=mkdtempSync(join(tmpdir(),'mcp-gate release Юникод '));
try {
  const sums=(await download('SHA256SUMS')).toString('utf8');
  const digest=sums.split('\n').find(line=>line.endsWith(`  ${filename}`))?.split('  ')[0];
  assert.match(digest??'',/^[0-9a-f]{64}$/);
  const archive=await download(filename);
  assert.equal(createHash('sha256').update(archive).digest('hex'),digest);
  const path=join(directory,filename);writeFileSync(path,archive);
  const listing=spawnSync('tar',['-tzf',path],{encoding:'utf8'});assert.equal(listing.status,0,listing.stderr);
  const exe=`mcp-gate${process.platform==='win32'?'.exe':''}`;
  const allow=new Set(['bin/','bin',`bin/${exe}`,'README.md','LICENSE']);
  for (const name of listing.stdout.trim().split(/\r?\n/)) assert(allow.has(name),`Unexpected archive entry: ${name}`);
  const expanded=join(directory,'expanded');mkdirSync(expanded);
  const extracted=spawnSync('tar',['-xzf',path,'-C',expanded],{encoding:'utf8'});assert.equal(extracted.status,0,extracted.stderr);
  const result=spawnSync(join(expanded,'bin',exe),['--version'],{encoding:'utf8',timeout:15_000});
  assert.equal(result.status,0,result.stderr);assert.equal(result.stdout.trim(),`mcp-gate ${version}`);
  console.log(`Verified downloaded GitHub archive: ${target}`);
} finally {rmSync(directory,{recursive:true,force:true});}
