import { readdir, readFile } from 'node:fs/promises';
import { resolve } from 'node:path';
import { spawnSync } from 'node:child_process';
const root=resolve(import.meta.dirname,'..');
let count=0;
for(const directory of ['src','public','scripts','test'])for(const name of await readdir(resolve(root,directory))){
  if(!/\.(mjs|js)$/.test(name))continue;
  const result=spawnSync(process.execPath,['--check',resolve(root,directory,name)],{encoding:'utf8'});
  if(result.status!==0){process.stderr.write(result.stderr);process.exitCode=1;}count++;
}
const catalog=JSON.parse(await readFile(resolve(root,'catalog/models.json'),'utf8'));
if(new Set(catalog.models.map(m=>m.id)).size!==catalog.models.length)throw new Error('Duplicate catalog id');
for(const model of catalog.models)if(!model.source.startsWith('https://')||!['keep','explore','skip'].includes(model.recommendation))throw new Error('Invalid model catalog entry');
process.stdout.write(`Checked ${count} JavaScript modules and ${catalog.models.length} model catalog entries.\n`);
