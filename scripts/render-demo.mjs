import { mkdir, writeFile } from 'node:fs/promises';
import { newProject } from '../src/project.mjs';
import { compose } from '../src/composer.mjs';
import { renderProject } from '../src/wav.mjs';
import { exportSMF, exportUMPTrace } from '../src/midi.mjs';
const project=newProject('Midnight gardens · procedural sketch');project.tracks=compose({bars:4,seed:42,density:.55}).tracks;
await mkdir(new URL('../dist/',import.meta.url),{recursive:true});
for(const [name,bytes]of [['midnight-gardens.wav',renderProject(project)],['midnight-gardens.mid',exportSMF(project).bytes],['midnight-gardens.ump.json',JSON.stringify(exportUMPTrace(project),null,2)],['midnight-gardens.project.json',JSON.stringify(project,null,2)]])await writeFile(new URL('../dist/'+name,import.meta.url),bytes);
process.stdout.write('Rendered the procedural demo to dist/. No AI model was used.\n');
