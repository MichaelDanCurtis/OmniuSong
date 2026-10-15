import { mkdir, readdir, rename, access } from 'node:fs/promises';
import { join } from 'node:path';
import { isDeepStrictEqual } from 'node:util';
import { atomicJSON, Conflict } from './storage.mjs';
import { copy, demand, newProject, validateProject } from './project.mjs';
import { sameTakeSource } from '../public/recording.js';

export const RECOVERY_LIMITS=Object.freeze({entries:16,declaredAudioBytes:128*1024*1024,parkedEntries:64,parkedDeclaredAudioBytes:512*1024*1024,metadataBytes:64*1024});
export class TakeRecovery {
  constructor(store){this.store=store;}
  async init(){for(const folder of ['pending-takes','parked-takes','completed-takes'])await mkdir(join(this.store.root,folder),{recursive:true,mode:0o700});return this;}
  validate(id,input){
    this.store.id(id);this.store.id(input?.projectId);
    demand(input.track?.id===id&&input.track.kind==='audio'&&input.track.assetId,'Recovery needs an original audio track');
    demand(['microphone','import'].includes(input.track.audio?.origin),'Only original takes and imports enter recovery');
    demand(Buffer.byteLength(JSON.stringify({projectId:input.projectId,track:input.track}))<=RECOVERY_LIMITS.metadataBytes,'Recovery metadata exceeds 64 KiB');
    const fixture=newProject('Recovery validation');fixture.tracks=[input.track];validateProject(fixture);
    return {projectId:input.projectId,track:copy(input.track)};
  }
  async find(id){
    for(const folder of ['pending-takes','parked-takes','completed-takes'])try{return {folder,entry:await this.store.read(folder,id)};}catch(error){if(error.code!=='ENOENT')throw error;}
    return null;
  }
  stage(id,input){return this.store.serial(async()=>{
    const proposal=this.validate(id,input),found=await this.find(id);
    if(found){if(!isDeepStrictEqual({projectId:found.entry.projectId,track:found.entry.track},proposal))throw new Conflict('This take identity already has a different recovery original');if(found.folder==='parked-takes')throw new Conflict('Return this set-aside original to recovery before retrying its save');return found.entry;}
    await this.store.read('projects',proposal.projectId);
    await this.capacity('pending-takes',proposal.track.audio.byteLength);
    const entry={version:1,id,createdAt:new Date().toISOString(),...proposal};
    await atomicJSON(join(this.store.root,'pending-takes',id+'.json'),entry);return entry;
  });}
  async capacity(folder,additionalBytes){
    const names=(await readdir(join(this.store.root,folder))).filter(n=>/^[a-zA-Z0-9-]{1,80}\.json$/.test(n)),parked=folder==='parked-takes';let bytes=0;
    for(const name of names){try{const entry=await this.store.read(folder,name.slice(0,-5));this.validate(name.slice(0,-5),entry);bytes+=entry.track.audio.byteLength;}catch{bytes+=30*1024*1024;}}
    demand(names.length<(parked?RECOVERY_LIMITS.parkedEntries:RECOVERY_LIMITS.entries),parked?'Set-aside recovery has reached its 64-entry limit':'Recover or set aside earlier takes before adding more than 16 unfinished saves');
    demand(bytes+additionalBytes<=(parked?RECOVERY_LIMITS.parkedDeclaredAudioBytes:RECOVERY_LIMITS.declaredAudioBytes),parked?'Set-aside originals exceed the 512 MiB recovery budget':'Unfinished saves exceed the 128 MiB recovery budget');
  }
  async list({includeParked=true}={}){
    const entries=[];
    for(const folder of includeParked?['pending-takes','parked-takes']:['pending-takes'])for(const name of (await readdir(join(this.store.root,folder))).filter(n=>/^[a-zA-Z0-9-]{1,80}\.json$/.test(n))){
      const parked=folder==='parked-takes';
      const id=name.slice(0,-5);let entry;
      try{entry=await this.store.read(folder,id);demand(entry.version===1&&entry.id===id,'Invalid recovery journal');this.validate(id,entry);}
      catch(error){if(error.code==='ENOENT')continue;entries.push({id,parked,state:'damaged-metadata',message:'Recovery metadata could not be read. The local files were retained.'});continue;}
      let state='ready',message='Original audio is saved locally; session attachment is unfinished.',projectName;
      try{const bytes=await this.store.asset(entry.track.assetId);demand(bytes.length===entry.track.audio.byteLength,'Original byte length does not match recovery metadata');}
      catch(error){state=error.code==='ENOENT'?'waiting-for-audio':'damaged-audio';message=state==='waiting-for-audio'?'Recovery metadata exists, but the original audio upload did not finish.':'Original audio failed its checksum or length check. Files were retained.';}
      try{
        const project=await this.store.read('projects',entry.projectId);projectName=project.name;
        const existing=project.tracks.find(track=>track.id===id);
        if(state==='ready'&&existing){state=sameTakeSource(existing,entry.track)?'already-attached':'identity-conflict';message=state==='already-attached'?'The session already contains this original. Finish recovery without adding a duplicate.':'A different track uses this identity. Download the retained original before reviewing the session.';}
      }catch(error){if(error.code!=='ENOENT')throw error;if(state==='ready'){state='session-missing';message='The original session is missing. Download the retained original.';}}
      entries.push({...entry,parked,state,message,projectName});
    }
    return entries.sort((a,b)=>Number(a.parked)-Number(b.parked)||(a.createdAt??'').localeCompare(b.createdAt??''));
  }
  move(id,action){return this.store.serial(async()=>{
    this.store.id(id);demand(['park','resume'].includes(action),'Invalid recovery action');
    const from=action==='park'?'pending-takes':'parked-takes',to=action==='park'?'parked-takes':'pending-takes';
    const source=join(this.store.root,from,id+'.json'),destination=join(this.store.root,to,id+'.json');
    // Check paths without parsing: even malformed journals must be reversible.
    try{await access(source);}catch(error){if(error.code!=='ENOENT')throw error;await access(destination);return {id,parked:action==='park'};}
    let declaredBytes=30*1024*1024,entry;
    try{entry=await this.store.read(from,id);this.validate(id,entry);declaredBytes=entry.track.audio.byteLength;}catch{entry=null;}
    if(entry&&action==='park'){
      let project;try{project=await this.store.read('projects',entry.projectId);}catch(error){if(error.code!=='ENOENT')throw error;}
      if(sameTakeSource(project?.tracks.find(track=>track.id===id),entry.track))throw new Conflict('This original is already saved in its session. Finish recovery instead.');
    }
    await this.capacity(to,declaredBytes);
    try{await access(destination);throw new Conflict('Another recovery entry already occupies this identity');}catch(error){if(error.code!=='ENOENT')throw error;}
    await rename(source,destination);return {id,parked:action==='park'};
  });}
  complete(id){return this.store.serial(async()=>{
    const found=await this.find(id);demand(found,'Recovery entry not found');
    const {entry,folder}=found;this.validate(id,entry);
    demand(folder!=='parked-takes','Return this set-aside original to recovery before completing it');
    const project=await this.store.read('projects',entry.projectId),track=project.tracks.find(t=>t.id===id);
    demand(sameTakeSource(track,entry.track),'Save the original in its session before completing recovery');
    const bytes=await this.store.asset(entry.track.assetId);demand(bytes.length===entry.track.audio.byteLength,'Original byte length does not match recovery metadata');
    if(folder==='pending-takes')await rename(join(this.store.root,folder,id+'.json'),join(this.store.root,'completed-takes',id+'.json'));
    return {id,completed:true};
  });}
}
