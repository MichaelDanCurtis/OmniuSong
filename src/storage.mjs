import { mkdir, readFile, rename, open, readdir, copyFile, unlink } from 'node:fs/promises';
import { join } from 'node:path';
import { createHash, randomUUID } from 'node:crypto';
import { validateProject, copy, demand, ASSET_ID } from './project.mjs';

export class Conflict extends Error { constructor(message='Project changed. Reload before saving.') {super(message);this.status=409;} }
export async function atomicWrite(path,value) {
  const tmp=`${path}.${randomUUID()}.tmp`;
  try {
    const handle=await open(tmp,'wx',0o600);
    try {await handle.writeFile(value);await handle.sync();} finally {await handle.close();}
    await rename(tmp,path);
  } finally {await unlink(tmp).catch(error=>{if(error.code!=='ENOENT')throw error;});}
}
export const atomicJSON=(path,value)=>atomicWrite(path,JSON.stringify(value,null,2)+'\n');
export class Store {
  constructor(root) {this.root=root;this.tail=Promise.resolve();}
  async init() {for(const name of ['projects','assets','jobs','backups']) await mkdir(join(this.root,name),{recursive:true,mode:0o700});return this;}
  serial(fn) {const op=this.tail.then(fn);this.tail=op.catch(()=>{});return op;}
  id(id) {demand(typeof id==='string'&&/^[a-zA-Z0-9-]{1,80}$/.test(id),'Invalid id');return id;}
  async read(folder,id) {return JSON.parse(await readFile(join(this.root,folder,this.id(id)+'.json'),'utf8'));}
  async list(folder) {const names=(await readdir(join(this.root,folder))).filter(n=>n.endsWith('.json'));return Promise.all(names.map(n=>this.read(folder,n.slice(0,-5))));}
  saveProject(project,expectedRevision) {return this.serial(async()=>{
    validateProject(project);
    let current;try {current=await this.read('projects',project.id);} catch(e) {if(e.code!=='ENOENT')throw e;}
    if(current&&current.revision!==expectedRevision)throw new Conflict();
    if(!current&&expectedRevision!==null)throw new Conflict('Project no longer exists');
    const next=copy(project);next.revision=current?current.revision+1:0;next.updatedAt=new Date().toISOString();
    if(current)await copyFile(join(this.root,'projects',next.id+'.json'),join(this.root,'backups',next.id+'.previous.json'));
    await atomicJSON(join(this.root,'projects',next.id+'.json'),next);return next;
  });}
  async saveJob(job) {await atomicJSON(join(this.root,'jobs',this.id(job.id)+'.json'),job);return job;}
  async putAsset(bytes,extension='wav') {
    demand(['wav','webm','mp4','ogg','mp3','flac'].includes(extension),'Unsupported audio container');
    const id=createHash('sha256').update(bytes).digest('hex')+'.'+extension;
    const path=join(this.root,'assets',id);
    let existing;try {existing=await readFile(path);}catch(error){if(error.code!=='ENOENT')throw error;}
    // A validated restore also repairs an interrupted or damaged copy of the
    // same content address. Complete bytes are committed before a project refers to them.
    if(!existing?.equals(bytes))await atomicWrite(path,bytes);
    return id;
  }
  async asset(id) {
    demand(ASSET_ID.test(id),'Invalid asset id');const bytes=await readFile(join(this.root,'assets',id));
    demand(createHash('sha256').update(bytes).digest('hex')===id.split('.')[0],`Audio asset is damaged: ${id}`);return bytes;
  }
}
