import test from 'node:test';
import assert from 'node:assert/strict';
import { mkdtemp, rm, readFile, writeFile, unlink } from 'node:fs/promises';
import { join } from 'node:path';
import { tmpdir } from 'node:os';
import { createHash } from 'node:crypto';
import { Store } from '../src/storage.mjs';
import { TakeRecovery, RECOVERY_LIMITS } from '../src/take-recovery.mjs';
import { newProject } from '../src/project.mjs';
import { createServer } from '../src/server.mjs';
import { TakeCapture, takeAssetId, sameTakeSource } from '../public/recording.js';
import { exportArchive, importArchive, inspectArchive } from '../src/archive.mjs';

const bytes=Buffer.from('complete procedural test audio bytes'),assetId=createHash('sha256').update(bytes).digest('hex')+'.wav';
function track(id='original-take'){return {id,name:'Retained original',kind:'audio',instrument:'voice',gain:.7,muted:false,locked:false,notes:[],assetId,audio:{origin:'import',mimeType:'audio/wav',durationSeconds:3.2,byteLength:bytes.length,originalName:'Test.wav',startTick:7680,external:{keep:true}}};}
async function fixture(t){const dir=await mkdtemp(join(tmpdir(),'omniusong-take-recovery-'));t.after(()=>rm(dir,{recursive:true,force:true}));const store=await new Store(dir).init(),recovery=await new TakeRecovery(store).init(),project=await store.saveProject(newProject('Recovery session'),null);return {dir,store,recovery,project,proposal:{projectId:project.id,track:track()}};}
test('browser content identity uses the original bytes and canonical audio container',async()=>{
  assert.equal(await takeAssetId(new Blob([bytes],{type:'audio/wav'}),'audio/wav'),assetId);
  assert.equal(await takeAssetId(new Blob([bytes]),'audio/mp4'),assetId.replace('.wav','.mp4'));
  await assert.rejects(takeAssetId(new Blob([]),'audio/wav'));await assert.rejects(takeAssetId(new Blob([bytes]),'image/png'));
  const original=track();assert.equal(sameTakeSource({...original,name:'Renamed',muted:true,audio:{...original.audio,startTick:99,trimStartSeconds:.1}},original),true);
  assert.equal(sameTakeSource({...original,kind:'notes'},original),false);assert.equal(sameTakeSource({...original,audio:{...original.audio,durationSeconds:9}},original),false);
});
test('journaling before upload survives a restart; completion requires saved original and remains idempotent',async t=>{
  const f=await fixture(t),entry=await f.recovery.stage('original-take',f.proposal);assert.equal((await f.recovery.list())[0].state,'waiting-for-audio');assert.equal(entry.track.audio.external.keep,true);
  await assert.rejects(f.recovery.complete(entry.id),/Save the original/);await f.store.putAsset(bytes,'wav');
  const restarted=await new TakeRecovery(await new Store(f.dir).init()).init();assert.equal((await restarted.list())[0].state,'ready');
  f.project.tracks.push(f.proposal.track);const saved=await f.store.saveProject(f.project,f.project.revision);await restarted.complete(entry.id);await restarted.complete(entry.id);assert.deepEqual(await restarted.list(),[]);assert.equal(JSON.parse(await readFile(join(f.dir,'completed-takes',entry.id+'.json'),'utf8')).track.assetId,assetId);assert.deepEqual(await f.store.asset(assetId),bytes);
  const destination=await new Store(join(f.dir,'restored')).init(),restored=await importArchive(destination,await exportArchive(f.store,saved));assert.deepEqual(restored.project.tracks,saved.tracks);assert.deepEqual(await destination.asset(assetId),bytes);
});
test('same-identity retries are safe under concurrency and conflicting originals cannot overwrite them',async t=>{
  const f=await fixture(t);const results=await Promise.all([f.recovery.stage('original-take',f.proposal),f.recovery.stage('original-take',{track:{...f.proposal.track},projectId:f.project.id})]);assert.deepEqual(results[0],results[1]);assert.equal((await f.recovery.list()).length,1);
  const changed=structuredClone(f.proposal);changed.track.name='Different original';await assert.rejects(f.recovery.stage('original-take',changed),/different recovery original/);assert.equal((await f.recovery.list())[0].track.name,'Retained original');
  for(const bad of [{...f.proposal,projectId:'../escape'},{...f.proposal,track:{...track(),kind:'notes'}},{...f.proposal,track:{...track(),audio:{...track().audio,origin:'generated'}}}])await assert.rejects(f.recovery.stage('original-take',bad));
});
test('recovery detects identity conflicts, damaged or absent media, missing sessions and malformed journals',async t=>{
  const f=await fixture(t);await f.recovery.stage('original-take',f.proposal);await f.store.putAsset(bytes,'wav');
  f.project.tracks.push({...track(),assetId:'a'.repeat(64)+'.wav'});await f.store.saveProject(f.project,0);assert.equal((await f.recovery.list())[0].state,'identity-conflict');await assert.rejects(f.recovery.complete('original-take'),/Save the original/);
  await unlink(join(f.dir,'projects',f.project.id+'.json'));assert.equal((await f.recovery.list())[0].state,'session-missing');
  await writeFile(join(f.dir,'assets',assetId),'damaged');assert.equal((await f.recovery.list())[0].state,'damaged-audio');
  await unlink(join(f.dir,'assets',assetId));assert.equal((await f.recovery.list())[0].state,'waiting-for-audio');
  await writeFile(join(f.dir,'pending-takes','original-take.json'),'{broken');assert.equal((await f.recovery.list())[0].state,'damaged-metadata');assert.equal(await readFile(join(f.dir,'pending-takes','original-take.json'),'utf8'),'{broken');
});
test('pending-take count, declared audio and metadata limits bound retained recovery work',async t=>{
  const f=await fixture(t);for(let i=0;i<RECOVERY_LIMITS.entries;i++)await f.recovery.stage('take-'+i,{projectId:f.project.id,track:track('take-'+i)});
  await assert.rejects(f.recovery.stage('overflow',{projectId:f.project.id,track:track('overflow')}),/16 unfinished/);
  const g=await fixture(t);for(let i=0;i<4;i++){const next=track('large-'+i);next.audio.byteLength=30*1024*1024;await g.recovery.stage(next.id,{projectId:g.project.id,track:next});}
  const large=track('large-4');large.audio.byteLength=30*1024*1024;await assert.rejects(g.recovery.stage(large.id,{projectId:g.project.id,track:large}),/128 MiB/);
  const metadata=track('metadata');metadata.audio.external={value:'x'.repeat(RECOVERY_LIMITS.metadataBytes)};await assert.rejects(g.recovery.stage(metadata.id,{projectId:g.project.id,track:metadata}),/64 KiB/);
});
test('recovery HTTP flow preserves original media and blocks premature acknowledgment and stale project saves',async t=>{
  const dir=await mkdtemp(join(tmpdir(),'omniusong-recovery-http-')),app=await createServer({dataDir:dir});await new Promise(r=>app.server.listen(0,'127.0.0.1',r));t.after(async()=>{await app.queue.close();await new Promise(r=>app.server.close(r));await rm(dir,{recursive:true,force:true});});
  const base='http://127.0.0.1:'+app.server.address().port,get=async path=>(await fetch(base+path)).json(),send=(path,value,method='POST',headers={})=>fetch(base+path,{method,headers:{'Content-Type':'application/json',...headers},body:JSON.stringify(value)}),project=await get('/api/projects/'+(await get('/api/projects'))[0].id),proposal={projectId:project.id,track:track()};
  assert.equal((await send('/api/pending-takes/original-take',proposal,'PUT',{Origin:'https://other.example'})).status,403);
  assert.equal((await send('/api/pending-takes/original-take',proposal,'PUT')).status,200);assert.equal((await get('/api/pending-takes'))[0].state,'waiting-for-audio');
  const upload=await fetch(base+'/api/assets',{method:'POST',headers:{'Content-Type':'audio/wav'},body:bytes});assert.equal((await upload.json()).assetId,assetId);assert.equal((await get('/api/pending-takes'))[0].state,'ready');
  assert.equal((await fetch(base+'/api/projects/'+project.id+'/archive')).status,400);
  assert.equal((await send('/api/pending-takes/original-take/park',{})).status,200);assert.equal((await get('/api/pending-takes'))[0].parked,true);
  const withoutOriginal=await fetch(base+'/api/projects/'+project.id+'/archive');assert.equal(withoutOriginal.status,200);
  const excluded=await inspectArchive(Buffer.from(await withoutOriginal.arrayBuffer()));assert.equal(excluded.assets.length,0);assert.deepEqual(excluded.project.tracks,project.tracks);
  assert.equal((await send('/api/pending-takes/original-take/resume',{})).status,200);assert.equal((await fetch(base+'/api/projects/'+project.id+'/archive')).status,400);
  assert.equal((await send('/api/pending-takes/original-take/complete',{})).status,400);
  project.tracks.push(proposal.track);assert.equal((await send('/api/projects/'+project.id,{project,expectedRevision:999},'PUT')).status,409);assert.equal((await get('/api/pending-takes')).length,1);
  assert.equal((await send('/api/projects/'+project.id,{project,expectedRevision:0},'PUT')).status,200);assert.equal((await get('/api/pending-takes'))[0].state,'already-attached');
  assert.equal((await send('/api/pending-takes/original-take/park',{})).status,409);
  assert.equal((await fetch(base+'/api/projects/'+project.id+'/archive')).status,200);
  for(let i=0;i<2;i++)assert.equal((await send('/api/pending-takes/original-take/complete',{})).status,200);assert.deepEqual(await get('/api/pending-takes'),[]);assert.equal((await get('/api/projects/'+project.id)).tracks.filter(t=>t.id==='original-take').length,1);
  assert.deepEqual(Buffer.from(await(await fetch(base+'/api/assets/'+assetId)).arrayBuffer()),bytes);
  assert.equal((await send('/api/pending-takes/oversized',{value:'x'.repeat(65536)},'PUT')).status,413);
});
test('recovered microphone takes retain origin and proposed identity through a failed save and retry',async()=>{
  let calls=0;const contexts=[],blob=new Blob([bytes],{type:'audio/wav'}),proposedTrack=track();proposedTrack.audio.origin='microphone';
  const capture=new TakeCapture({getStream:()=>{throw new Error('Recovery must not request a microphone');},createRecorder:()=>{throw new Error('Recovery must not encode again');},persist:async(original,context)=>{assert.equal(original,blob);contexts.push(structuredClone(context));if(++calls===1)throw new Error('session conflict');}});
  const context={origin:'microphone',projectId:'same-project',takeId:proposedTrack.id,assetId,assetUploaded:true,recoverySaved:true,proposedTrack};await capture.recover(blob,context);assert.equal(capture.phase,'failed');context.proposedTrack.name='Changed outside';await capture.retry();assert.equal(capture.phase,'idle');assert.deepEqual(contexts[0],contexts[1]);assert.equal(contexts[1].origin,'microphone');assert.equal(contexts[1].proposedTrack.name,'Retained original');
});
test('set-aside and return preserve exact journals, originals and projects; retries cannot silently reactivate them',async t=>{
  const f=await fixture(t);await f.recovery.stage('original-take',f.proposal);await f.store.putAsset(bytes,'wav');const raw=await readFile(join(f.dir,'pending-takes','original-take.json'));
  await f.recovery.move('original-take','park');await f.recovery.move('original-take','park');assert.equal((await f.recovery.list())[0].parked,true);assert.deepEqual(await f.recovery.list({includeParked:false}),[]);assert.deepEqual(await readFile(join(f.dir,'parked-takes','original-take.json')),raw);assert.deepEqual(await f.store.asset(assetId),bytes);assert.deepEqual(await f.store.read('projects',f.project.id),f.project);
  await assert.rejects(f.recovery.stage('original-take',f.proposal),/Return this set-aside/);await assert.rejects(f.recovery.complete('original-take'),/Return this set-aside/);
  const restarted=await new TakeRecovery(await new Store(f.dir).init()).init();assert.equal((await restarted.list())[0].parked,true);await restarted.move('original-take','resume');await restarted.move('original-take','resume');assert.equal((await restarted.list())[0].parked,false);assert.deepEqual(await readFile(join(f.dir,'pending-takes','original-take.json')),raw);
  f.project.tracks.push(f.proposal.track);await f.store.saveProject(f.project,0);await restarted.complete('original-take');await assert.rejects(restarted.move('original-take','park'));assert.deepEqual(await restarted.list(),[]);
});
test('malformed metadata can be set aside and returned without alteration or invented media recovery',async t=>{
  const f=await fixture(t);await f.recovery.stage('original-take',f.proposal);const raw=Buffer.from('{unreadable journal');await writeFile(join(f.dir,'pending-takes','original-take.json'),raw);
  await f.recovery.move('original-take','park');const entry=(await f.recovery.list())[0];assert.equal(entry.parked,true);assert.equal(entry.state,'damaged-metadata');assert.equal(entry.track,undefined);assert.deepEqual(await readFile(join(f.dir,'parked-takes','original-take.json')),raw);
  await f.recovery.move('original-take','resume');assert.equal((await f.recovery.list())[0].parked,false);assert.deepEqual(await readFile(join(f.dir,'pending-takes','original-take.json')),raw);
});
test('returning to recovery respects active limits; set-aside limits retain the source on failed moves',async t=>{
  const f=await fixture(t);for(let i=0;i<16;i++)await f.recovery.stage('take-'+i,{projectId:f.project.id,track:track('take-'+i)});await f.recovery.move('take-0','park');await f.recovery.stage('replacement',{projectId:f.project.id,track:track('replacement')});await assert.rejects(f.recovery.move('take-0','resume'),/16 unfinished/);assert.equal((await f.recovery.list()).find(e=>e.id==='take-0').parked,true);
  await f.recovery.move('take-1','park');await f.recovery.move('take-0','resume');assert.equal((await f.recovery.list()).filter(e=>!e.parked).length,16);
  const g=await fixture(t);for(let i=0;i<65;i++){const id='park-'+i;await g.recovery.stage(id,{projectId:g.project.id,track:track(id)});if(i<64)await g.recovery.move(id,'park');}await assert.rejects(g.recovery.move('park-64','park'),/64-entry/);assert.equal((await g.recovery.list({includeParked:false}))[0].id,'park-64');
  const h=await fixture(t);for(let i=0;i<18;i++){const item=track('large-park-'+i);item.audio.byteLength=30*1024*1024;await h.recovery.stage(item.id,{projectId:h.project.id,track:item});if(i<17)await h.recovery.move(item.id,'park');}await assert.rejects(h.recovery.move('large-park-17','park'),/512 MiB/);assert.equal((await h.recovery.list({includeParked:false}))[0].id,'large-park-17');
});
