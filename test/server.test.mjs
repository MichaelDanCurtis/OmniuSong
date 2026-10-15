import test from 'node:test';
import assert from 'node:assert/strict';
import { mkdtemp, rm } from 'node:fs/promises';
import { join } from 'node:path';
import { tmpdir } from 'node:os';
import { createServer } from '../src/server.mjs';

test('studio API supports real persistence, review, export and origin isolation',async t=>{
  const dir=await mkdtemp(join(tmpdir(),'omniusong-api-')),app=await createServer({dataDir:dir});
  await new Promise(r=>app.server.listen(0,'127.0.0.1',r));
  t.after(async()=>{await app.queue.close();await new Promise(r=>app.server.close(r));await rm(dir,{recursive:true,force:true});});
  const base='http://127.0.0.1:'+app.server.address().port;
  const get=async path=>(await fetch(base+path)).json();
  const send=async(path,data,method='POST',headers={})=>fetch(base+path,{method,headers:{'Content-Type':'application/json',...headers},body:JSON.stringify(data)});
  const projects=await get('/api/projects');assert.equal(projects.length,1);const p=await get('/api/projects/'+projects[0].id);
  const catalog=await get('/api/catalog');assert.ok(catalog.models.length>=30);
  const page=await fetch(base+'/');assert.equal(page.status,200);assert.match(await page.text(),/piano-roll/);
  const noteView=await fetch(base+'/note-view.js');assert.equal(noteView.status,200);assert.match(noteView.headers.get('content-type'),/javascript/);
  const noteEdit=await fetch(base+'/note-edit.js');assert.equal(noteEdit.status,200);assert.match(noteEdit.headers.get('content-type'),/javascript/);
  assert.equal((await fetch(base+'/clip-edit.js')).status,200);
  assert.equal((await get('/api/status')).ariaStudioControls,true);
  assert.equal((await get('/api/status')).magentaStudioControls,true);
  assert.ok((await get('/api/plugins')).some(p=>p.id==='eleutherai.aria-mlx'&&p.ai&&p.capabilities.includes('continue')));
  const blocked=await send('/api/projects',{name:'bad'},'POST',{Origin:'https://untrusted.example'});assert.equal(blocked.status,403);
  const old=p.revision;p.name='A real saved title';const savedResponse=await send('/api/projects/'+p.id,{project:p,expectedRevision:old},'PUT');assert.equal(savedResponse.status,200);const saved=await savedResponse.json();assert.equal(saved.revision,1);
  const conflict=await send('/api/projects/'+p.id,{project:p,expectedRevision:old},'PUT');assert.equal(conflict.status,409);
  const created=await send('/api/jobs',{projectId:p.id,projectRevision:1,pluginId:'local.sketch',capability:'compose',input:{seed:123,density:.4}});assert.equal(created.status,202);const job=await created.json();
  for(let i=0;i<100;i++){if(app.queue.jobs.get(job.id).state==='succeeded')break;await new Promise(r=>setTimeout(r,10));}
  const accepted=await send('/api/jobs/'+job.id+'/accept',{expectedRevision:1});assert.equal(accepted.status,200);
  const midi=await fetch(base+'/api/projects/'+p.id+'/midi');assert.equal(midi.headers.get('content-type'),'audio/midi');assert.equal(Buffer.from(await midi.arrayBuffer()).toString('ascii',0,4),'MThd');
  const ump=await get('/api/projects/'+p.id+'/ump');assert.equal(ump.protocol,'MIDI 2.0');assert.ok(ump.packets.length>0);
  const badAsset=await fetch(base+'/api/assets/..%2Fprivate');assert.equal(badAsset.status,404);
  const exampleResponse=await fetch(base+'/api/examples/four-tones.wav'),example=Buffer.from(await exampleResponse.arrayBuffer());assert.equal(example.length,282284);
  const uploaded=await fetch(base+'/api/assets',{method:'POST',headers:{'Content-Type':'audio/wav'},body:example}),asset=await uploaded.json();assert.equal(uploaded.status,201);
  const complete=await fetch(base+'/api/assets/'+asset.assetId);assert.equal(Number(complete.headers.get('Content-Length')),example.length);assert.equal(complete.headers.get('Accept-Ranges'),'bytes');assert.deepEqual(Buffer.from(await complete.arrayBuffer()),example);
  for(const [range,start,end]of [['bytes=0-43',0,43],['bytes=-10',example.length-10,example.length-1],['bytes=100-',100,example.length-1]]){
    const partial=await fetch(base+'/api/assets/'+asset.assetId,{headers:{Range:range}});assert.equal(partial.status,206);assert.equal(partial.headers.get('Content-Range'),`bytes ${start}-${end}/${example.length}`);assert.deepEqual(Buffer.from(await partial.arrayBuffer()),example.subarray(start,end+1));
  }
  for(const range of ['bytes=999999-','bytes=0-1,4-5','bytes=-0','bytes=9-3'])assert.equal((await fetch(base+'/api/assets/'+asset.assetId,{headers:{Range:range}})).status,416);
  for(const [mime,extension]of [['audio/mpeg','mp3'],['audio/flac','flac']]){
    const result=await(await fetch(base+'/api/assets',{method:'POST',headers:{'Content-Type':mime},body:Buffer.from('container-byte-preservation fixture')})).json();assert.ok(result.assetId.endsWith('.'+extension));
    const original=await fetch(base+'/api/assets/'+result.assetId);assert.equal(original.headers.get('Content-Type'),mime);assert.equal(await original.text(),'container-byte-preservation fixture');
  }
  const selections=await send('/api/selections',{'aria':'keep'},'PUT');assert.equal(selections.status,200);assert.equal((await get('/api/selections')).aria,'keep');
});
