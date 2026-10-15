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
  const blocked=await send('/api/projects',{name:'bad'},'POST',{Origin:'https://untrusted.example'});assert.equal(blocked.status,403);
  const old=p.revision;p.name='A real saved title';const savedResponse=await send('/api/projects/'+p.id,{project:p,expectedRevision:old},'PUT');assert.equal(savedResponse.status,200);const saved=await savedResponse.json();assert.equal(saved.revision,1);
  const conflict=await send('/api/projects/'+p.id,{project:p,expectedRevision:old},'PUT');assert.equal(conflict.status,409);
  const created=await send('/api/jobs',{projectId:p.id,projectRevision:1,pluginId:'local.sketch',capability:'compose',input:{seed:123,density:.4}});assert.equal(created.status,202);const job=await created.json();
  for(let i=0;i<100;i++){if(app.queue.jobs.get(job.id).state==='succeeded')break;await new Promise(r=>setTimeout(r,10));}
  const accepted=await send('/api/jobs/'+job.id+'/accept',{expectedRevision:1});assert.equal(accepted.status,200);
  const midi=await fetch(base+'/api/projects/'+p.id+'/midi');assert.equal(midi.headers.get('content-type'),'audio/midi');assert.equal(Buffer.from(await midi.arrayBuffer()).toString('ascii',0,4),'MThd');
  const ump=await get('/api/projects/'+p.id+'/ump');assert.equal(ump.protocol,'MIDI 2.0');assert.ok(ump.packets.length>0);
  const badAsset=await fetch(base+'/api/assets/..%2Fprivate');assert.equal(badAsset.status,404);
  const selections=await send('/api/selections',{'aria':'keep'},'PUT');assert.equal(selections.status,200);assert.equal((await get('/api/selections')).aria,'keep');
});
