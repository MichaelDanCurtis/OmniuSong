import test from 'node:test';
import assert from 'node:assert/strict';
import { readFile, mkdtemp, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { ariaInstalled, runAria } from '../src/worker.mjs';
import { newProject } from '../src/project.mjs';
import { Store } from '../src/storage.mjs';
import { createPlugins } from '../src/plugins.mjs';
import { JobQueue } from '../src/jobs.mjs';
import { exportArchive, importArchive } from '../src/archive.mjs';
import { candidateAudition } from '../public/note-view.js';

const installed=await ariaInstalled();
test('qualified Aria runs through the queue, acceptance and archive without changing the source',{skip:!installed,timeout:120000},async t=>{
  const request=JSON.parse(await readFile(new URL('../docs/verification/aria-prompt.json',import.meta.url),'utf8')),original=structuredClone(request);
  const dir=await mkdtemp(join(tmpdir(),'omniusong-aria-queue-'));t.after(()=>rm(dir,{recursive:true,force:true}));
  const store=await new Store(dir).init(),p=newProject('Aria queue fixture');p.tempo=request.tempo;p.tracks[0].locked=true;
  p.tracks[0].notes=request.notes.map((n,i)=>({...n,id:'source-'+i,expression:[]}));await store.saveProject(p,null);
  const source=structuredClone(p.tracks[0]),queue=await new JobQueue(store,createPlugins(store)).init();
  const job=await queue.submit({projectId:p.id,projectRevision:0,pluginId:'eleutherai.aria-mlx',capability:'continue',input:{sourceTrackId:source.id,startBar:1,phraseBars:2,seed:request.seed,maxNewTokens:request.maxNewTokens}});
  const deadline=Date.now()+100000;while(['queued','running'].includes(queue.jobs.get(job.id).state)&&Date.now()<deadline)await new Promise(r=>setTimeout(r,50));
  const completed=queue.jobs.get(job.id);assert.equal(completed.state,'succeeded',completed.error);const result=completed.result,evidence=result.tracks[0].continuation;
  assert.deepEqual(request,original);assert.equal(evidence.backend,'mlx');assert.equal(evidence.generatedTokens,64);
  assert.equal(evidence.decodedPromptNotes,request.notes.length);assert.ok(result.tracks[0].notes.length>=4);assert.equal(result.tracks[0].notes[0].startTick,7680);
  assert.ok(result.tracks[0].notes.every(n=>n.modelEstimate.velocity7>0&&n.modelEstimate.startMilliseconds>=evidence.firstGeneratedOnsetMilliseconds));
  assert.equal(result.modelRevision,'5a1ef36c5007ee10fbe57d61d96725a7244bd2b2');
  assert.equal((await store.read('projects',p.id)).tracks.length,2);assert.equal(candidateAudition(p,result).tracks[0].notes[0].startTick,0);
  const accepted=(await queue.decide(job.id,'accepted',0)).project;assert.deepEqual(accepted.tracks[0],source);
  assert.ok(accepted.bars>p.bars);assert.equal(accepted.history[0].provenance.modelSha256,result.modelSha256);
  const restored=(await importArchive(store,await exportArchive(store,accepted))).project;
  assert.deepEqual(restored.tracks,accepted.tracks);assert.equal(restored.bars,accepted.bars);
});
test('Aria rejects an invalid request and honors cancellation before inference',{skip:!installed,timeout:120000},async()=>{
  await assert.rejects(runAria({tempo:104,seed:0,maxNewTokens:257,notes:[]},new AbortController().signal),/Invalid seed or token limit/);
  const controller=new AbortController();controller.abort(new Error('Canceled fixture'));
  await assert.rejects(runAria({},controller.signal),/Canceled fixture/);
});
