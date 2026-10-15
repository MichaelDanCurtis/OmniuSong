import test from 'node:test';
import assert from 'node:assert/strict';
import { mkdtemp, rm, readdir, writeFile, readFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { gzipSync, gunzipSync } from 'node:zlib';
import { Store } from '../src/storage.mjs';
import { newProject } from '../src/project.mjs';
import { exportArchive, inspectArchive, importArchive, referencedAssets, ARCHIVE_LIMITS, ARCHIVE_MIME } from '../src/archive.mjs';
import { createServer } from '../src/server.mjs';

async function storeFor(t){const root=await mkdtemp(join(tmpdir(),'omniusong-archive-'));t.after(()=>rm(root,{recursive:true,force:true}));return new Store(root).init();}
async function fixture(t){
  const store=await storeFor(t),project=newProject('Original performance');
  const asset=await store.putAsset(Buffer.from('original voice recording'),'webm'),analysis=await store.putAsset(Buffer.from('decoded analysis audio'),'wav');
  project.tracks[0].assetId=asset;project.tracks[0].sourceAssetId=asset;project.tracks[0].locked=true;
  project.tracks[0].notes=[{id:'expressive-note',pitch:60,startTick:123,durationTicks:987,velocity16:49123,expression:[{type:'pitch',points:[{tick:0,value32:0x80000000},{tick:250,value32:0x87654321}]}],externalMetadata:{namespace:'retained'}}];
  project.history.push({id:'decision',provenance:{input:{assetId:analysis,sourceAssetId:asset}}});
  project.providerMetadata={custom:'untouched'};
  const saved=await store.saveProject(project,null),bytes=await exportArchive(store,saved);
  return {store,project:saved,bytes,asset,analysis};
}
const change=(bytes,fn)=>{const archive=JSON.parse(gunzipSync(bytes));fn(archive);return gzipSync(JSON.stringify(archive));};

test('complete backup restores into an empty store, retains expression and provenance, and creates a new identity',async t=>{
  const f=await fixture(t),destination=await storeFor(t),restored=await importArchive(destination,f.bytes);
  assert.notEqual(restored.project.id,f.project.id);assert.equal(restored.project.revision,0);assert.equal(restored.assetCount,2);
  assert.deepEqual(restored.project.tracks,f.project.tracks);assert.deepEqual(restored.project.history,f.project.history);assert.deepEqual(restored.project.providerMetadata,f.project.providerMetadata);
  assert.equal(restored.project.importedFrom.projectId,f.project.id);
  assert.deepEqual(await destination.asset(f.asset),await f.store.asset(f.asset));assert.deepEqual(await destination.asset(f.analysis),await f.store.asset(f.analysis));
  const exportedAgain=await inspectArchive(await exportArchive(destination,restored.project));assert.equal(exportedAgain.assets.length,2);
});
test('restore beside an existing project never overwrites it and deduplicates shared audio',async t=>{
  const f=await fixture(t),first=await importArchive(f.store,f.bytes),second=await importArchive(f.store,f.bytes);
  assert.equal((await f.store.list('projects')).length,3);assert.notEqual(first.project.id,second.project.id);assert.deepEqual(await f.store.read('projects',f.project.id),f.project);
  assert.equal((await readdir(join(f.store.root,'assets'))).length,2);
});
test('missing or damaged source audio prevents an apparently complete export',async t=>{
  const f=await fixture(t);await rm(join(f.store.root,'assets',f.asset));await assert.rejects(exportArchive(f.store,f.project),/is missing/);
  await writeFile(join(f.store.root,'assets',f.asset),'damaged');await assert.rejects(exportArchive(f.store,f.project),/damaged/);
});
test('validated restore repairs a damaged copy of the same content address',async t=>{
  const f=await fixture(t);await writeFile(join(f.store.root,'assets',f.asset),'damaged');await importArchive(f.store,f.bytes);
  assert.equal((await f.store.asset(f.asset)).toString(),'original voice recording');
});
test('malformed bundles fail before publishing any project or audio',async t=>{
  const f=await fixture(t),destination=await storeFor(t);
  for(const malformed of [Buffer.from('not an archive'),Buffer.from([31,139,0]),gzipSync('{broken'),change(f.bytes,a=>a.version=2),change(f.bytes,a=>a.assets.pop()),change(f.bytes,a=>a.assets.push(a.assets[0])),change(f.bytes,a=>a.assets[0].data='!'.repeat(a.assets[0].data.length)),change(f.bytes,a=>a.assets[0].id='../outside.wav'),change(f.bytes,a=>a.assets[0].data=Buffer.from('X'.repeat(a.assets[0].byteLength)).toString('base64'))]){
    await assert.rejects(importArchive(destination,malformed));
  }
  assert.deepEqual(await destination.list('projects'),[]);assert.deepEqual(await readdir(join(destination.root,'assets')),[]);
});
test('manifest rejects unused audio and oversized declared assets',async t=>{
  const f=await fixture(t);
  await assert.rejects(inspectArchive(change(f.bytes,a=>{a.project.history=[];a.assets.push({...a.assets[0],id:'0'.repeat(64)+'.wav'});})),/outside the project/);
  await assert.rejects(inspectArchive(change(f.bytes,a=>a.assets[0].byteLength=ARCHIVE_LIMITS.assetBytes+1)),/oversized/);
});
test('expanded gzip input is bounded before JSON parsing',async()=>{
  const oversized=gzipSync(Buffer.alloc(ARCHIVE_LIMITS.jsonBytes+1,32));
  await assert.rejects(inspectArchive(oversized),/Expanded archive exceeds/);
});
test('imported metadata leaves room for subsequent saves through the control service',async t=>{
  const f=await fixture(t);
  await assert.rejects(inspectArchive(change(f.bytes,a=>a.project.largeMetadata='x'.repeat(ARCHIVE_LIMITS.projectBytes))),/Project metadata exceeds/);
});
test('asset references cannot escape the content store and deeply nested metadata is bounded',()=>{
  assert.throws(()=>referencedAssets({provenance:{assetId:'../../outside.wav'}}),/Invalid asset/);
  assert.throws(()=>referencedAssets({provenance:{sourceAssetId:['0'.repeat(64)+'.wav']}}),/Invalid asset/);
  let nested={};for(let i=0;i<70;i++)nested={child:nested};assert.throws(()=>referencedAssets(nested),/deeply nested/);
});
test('HTTP backup download and restore preserve a real session and fail closed on damaged content',async t=>{
  const store=await storeFor(t),app=await createServer({dataDir:store.root});await new Promise(r=>app.server.listen(0,'127.0.0.1',r));t.after(async()=>{await app.queue.close();await new Promise(r=>app.server.close(r));});
  const base='http://127.0.0.1:'+app.server.address().port,projects=await (await fetch(base+'/api/projects')).json();
  const p=await store.read('projects',projects[0].id),assetId=await store.putAsset(Buffer.from('take'),'wav');p.tracks[0].assetId=assetId;await store.saveProject(p,0);
  const download=await fetch(base+'/api/projects/'+p.id+'/archive');assert.equal(download.status,200);assert.equal(download.headers.get('Content-Type'),ARCHIVE_MIME);const bytes=Buffer.from(await download.arrayBuffer());
  const restore=await fetch(base+'/api/archives',{method:'POST',headers:{'Content-Type':ARCHIVE_MIME},body:bytes});assert.equal(restore.status,201);const result=await restore.json();assert.notEqual(result.project.id,p.id);assert.equal(result.assetCount,1);
  const bad=await fetch(base+'/api/archives',{method:'POST',headers:{'Content-Type':ARCHIVE_MIME},body:Buffer.from('broken')});assert.equal(bad.status,422);assert.equal((await store.list('projects')).length,2);
  const forbidden=await fetch(base+'/api/archives',{method:'POST',headers:{'Content-Type':ARCHIVE_MIME,Origin:'https://untrusted.example'},body:bytes});assert.equal(forbidden.status,403);
});
