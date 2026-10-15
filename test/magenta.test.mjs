import test from 'node:test';
import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { readFile, mkdtemp, rm } from 'node:fs/promises';
import { join } from 'node:path';
import { tmpdir } from 'node:os';
import { magentaInstalled, magentaPython, runMagenta } from '../src/magenta.mjs';
import { Store } from '../src/storage.mjs';
import { newProject } from '../src/project.mjs';
import { exportArchive, importArchive } from '../src/archive.mjs';
import { createPlugins } from '../src/plugins.mjs';
import { JobQueue } from '../src/jobs.mjs';
import { candidateAudition } from '../public/note-view.js';

const installed=await magentaInstalled();
test('Magenta projects short, repeated and sustained notes at explicit 25 Hz without changing the source',{skip:!installed},()=>{
  const code=`import importlib.util, copy
spec=importlib.util.spec_from_file_location('worker','workers/magenta_worker.py'); module=importlib.util.module_from_spec(spec); spec.loader.exec_module(module)
request={'prompt':'synthetic fixture','seed':42,'durationSeconds':1,'tempo':60,'notes':[{'pitch':60,'startTick':40,'durationTicks':1},{'pitch':60,'startTick':80,'durationTicks':80}]}
before=copy.deepcopy(request); frames=module.conditioning_frames(module.validate_request(request))
assert request==before and len(frames)==25 and all(len(f)==128 for f in frames)
assert frames[1][60]==2 and frames[2][60]==2 and frames[3][60]==1 and frames[5][60]==0
assert module.conditioning_frames({**request,'notes':[]}) is None
for changes in [{'durationSeconds':9},{'seed':True},{'prompt':''},{'notes':[{'pitch':128,'startTick':0,'durationTicks':1}]},{'notes':[{'pitch':60,'startTick':0,'durationTicks':961}]}]:
 try: module.validate_request({**request,**changes})
 except ValueError: pass
 else: raise AssertionError(changes)
print('25 Hz projection and bounds verified')`;
  const result=spawnSync(magentaPython,['-c',code],{cwd:new URL('../',import.meta.url),encoding:'utf8'});assert.equal(result.status,0,result.stderr);
});
test('real Magenta Small runs through scoped candidates, acceptance and complete backups',{skip:!installed,timeout:120000},async t=>{
  const request=JSON.parse(await readFile(new URL('../docs/verification/magenta-prompt.json',import.meta.url),'utf8')),before=structuredClone(request);
  const dir=await mkdtemp(join(tmpdir(),'omniusong-magenta-test-'));t.after(()=>rm(dir,{recursive:true,force:true}));
  const store=await new Store(dir).init(),p=newProject('Generated Magenta qualification');p.bars=2;p.tempo=request.tempo;p.tracks[0].locked=true;
  p.tracks[0].notes=request.notes.map((n,i)=>({...n,id:'source'+i,startTick:n.startTick+3840}));const source=structuredClone(p.tracks[0]);await store.saveProject(p,null);
  const queue=await new JobQueue(store,createPlugins(store)).init(),job=await queue.submit({projectId:p.id,projectRevision:0,pluginId:'google.magenta-small',capability:'generate-audio',input:{prompt:request.prompt,seed:42,durationSeconds:4,startBar:2,guideWithNotes:true,sourceTrackId:source.id}});
  const deadline=Date.now()+100000;while(['queued','running'].includes(queue.jobs.get(job.id).state)&&Date.now()<deadline)await new Promise(r=>setTimeout(r,50));
  const completed=queue.jobs.get(job.id);assert.equal(completed.state,'succeeded',completed.error);const candidate=completed.result,result=candidate.tracks[0].generation,audio=await store.asset(candidate.tracks[0].assetId);
  assert.deepEqual(request,before);assert.equal(result.channels,2);assert.equal(result.sampleRate,48000);assert.equal(audio.length,768044);
  assert.equal(result.conditioning.sourceNoteCount,4);assert.equal(result.conditioning.trace.flatMap(f=>f.onsets).length,4);
  assert.ok(result.rmsAmplitude>0.001);assert.equal(result.clippedSampleCount,0);assert.equal(result.generationFrames,100);
  assert.equal((await store.read('projects',p.id)).tracks.length,2);assert.equal(candidateAudition(p,candidate).tracks[0].audio.startTick,0);
  const saved=(await queue.decide(job.id,'accepted',0)).project;assert.equal(saved.bars,3);assert.deepEqual(saved.tracks[0],source);assert.equal(saved.tracks.at(-1).audio.startTick,3840);
  assert.deepEqual(saved.tracks.at(-1).generation.scope.guideNotes,request.notes.map(({pitch,startTick,durationTicks})=>({pitch,startTick,durationTicks})));
  const restored=await importArchive(store,await exportArchive(store,saved));
  assert.deepEqual(restored.project.tracks,saved.tracks);assert.deepEqual(await store.asset(restored.project.tracks.at(-1).assetId),audio);
});
test('Magenta rejects over-budget and canceled requests before producing audio',{skip:!installed,timeout:10000},async()=>{
  await assert.rejects(runMagenta({prompt:'fixture',seed:42,durationSeconds:9},new AbortController().signal),/1–8 whole seconds/);
  const controller=new AbortController();controller.abort(new Error('Canceled fixture'));
  await assert.rejects(runMagenta({},controller.signal),/Canceled fixture/);
  const active=new AbortController(),timer=setTimeout(()=>active.abort(new Error('Canceled during startup')),50);
  try{await assert.rejects(runMagenta({prompt:'fixture',seed:42,durationSeconds:4},active.signal),error=>error.name==='AbortError');}finally{clearTimeout(timer);}
});
