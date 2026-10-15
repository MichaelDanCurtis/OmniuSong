import test from 'node:test';
import assert from 'node:assert/strict';
import { mkdtemp, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { newProject, mergeCandidate } from '../src/project.mjs';
import { ariaPhrase, ariaCandidate } from '../src/continuation.mjs';
import { candidateAudition, rollWindow } from '../public/note-view.js';
import { Store } from '../src/storage.mjs';
import { PluginRegistry } from '../src/plugins.mjs';
import { JobQueue } from '../src/jobs.mjs';

const note={id:'human',pitch:60,startTick:3700,durationTicks:5000,velocity16:51312,expression:[{type:'pressure',points:[{tick:100,value32:0xfedcba98}]}]};
function fixture(){const p=newProject();p.tracks[0].locked=true;p.tracks[0].notes=[structuredClone(note)];return p;}
function input(p,overrides={}){return {sourceTrackId:p.tracks[0].id,startBar:2,phraseBars:1,seed:42,maxNewTokens:64,...overrides};}
const result={model:'Aria fixture, no inference',modelSha256:'test-only',modelRevision:'test-only',generatedTokens:64,notes:[{id:'answer',pitch:90,startTick:0,durationTicks:17000,velocity16:40000,expression:[],modelEstimate:{startMilliseconds:1000,endMilliseconds:2000,velocity7:78}}]};
test('phrase scope crops context only and preserves locked high-resolution source notes',()=>{
  const p=fixture(),before=structuredClone(p),{request,scope}=ariaPhrase(p,input(p));
  assert.deepEqual(p,before);assert.deepEqual(request.notes,[{pitch:60,startTick:0,durationTicks:3840,velocity16:51312}]);
  assert.equal(scope.clippedNotes,1);assert.match(scope.projection,/expression is not used/);assert.equal(scope.sourceProjectRevision,0);
});
test('phrase validation rejects empty, oversized, non-pitched and out-of-session requests',()=>{
  const p=fixture();
  for(const changes of [{startBar:0},{startBar:4,phraseBars:2},{phraseBars:5},{seed:-1},{maxNewTokens:257},{sourceTrackId:p.tracks[1].id},{startBar:1,phraseBars:1,sourceTrackId:'missing'}])assert.throws(()=>ariaPhrase(p,input(p,changes)));
  p.tempo=30;assert.throws(()=>ariaPhrase(p,input(p,{phraseBars:2})),/15 seconds/);p.tempo=104;
  p.tracks[0].notes=[];assert.throws(()=>ariaPhrase(p,input(p)),/no notes/);
  p.tracks[0].notes=Array.from({length:257},(_,i)=>({...note,id:'n'+i}));assert.throws(()=>ariaPhrase(p,input(p)),/256 source notes/);
});
test('a continuation appends after the exact phrase, extends safely and retains model evidence',()=>{
  const p=fixture(),before=structuredClone(p),{scope}=ariaPhrase(p,input(p)),candidate=ariaCandidate(p,scope,result),next=mergeCandidate(p,candidate,{ai:true});
  assert.equal(candidate.minimumBars,7);assert.equal(next.bars,7);assert.equal(next.tracks.at(-1).notes[0].startTick,7680);
  assert.deepEqual(next.tracks[0],before.tracks[0]);assert.deepEqual(p,before);
  assert.equal(next.tracks.at(-1).continuation.modelSha256,'test-only');assert.equal(next.tracks.at(-1).notes[0].modelEstimate.startMilliseconds,1000);
  assert.equal(mergeCandidate({...p,bars:8},candidate,{}).bars,8);
  assert.throws(()=>mergeCandidate(p,{minimumBars:129},{}),/session length/);
  assert.throws(()=>mergeCandidate(p,{minimumBars:2.5},{}),/session length/);
  assert.throws(()=>ariaCandidate(p,{...scope,endTick:128*3840},result),/128-bar/);
});
test('audition starts at the answer, covers its full duration and leaves saved timing untouched',()=>{
  const p=fixture(),{scope}=ariaPhrase(p,input(p)),candidate=ariaCandidate(p,scope,result),before=structuredClone(candidate),preview=candidateAudition(p,candidate);
  assert.equal(preview.tracks[0].notes[0].startTick,0);assert.equal(preview.bars,5);assert.deepEqual(candidate,before);assert.equal(p.bars,4);
  assert.equal(candidateAudition(p,{tracks:[],minimumBars:1}).bars,4);
});
test('roll navigation covers every session bar and MIDI pitch at the edges',()=>{
  assert.deepEqual(rollWindow(7,1,127),{page:1,pages:2,startTick:15360,startBar:5,endBar:7,topPitch:127});
  assert.equal(rollWindow(128,999,1).page,31);assert.equal(rollWindow(1,-1,1).topPitch,24);
  assert.equal(rollWindow(128,31).endBar,128);assert.equal(rollWindow(1).endBar,1);
});
test('a source edited while queued fails before its model handler runs',async t=>{
  const dir=await mkdtemp(join(tmpdir(),'omniusong-queue-scope-'));t.after(()=>rm(dir,{recursive:true,force:true}));
  const store=await new Store(dir).init(),p=await store.saveProject(fixture(),null),registry=new PluginRegistry();let runs=0;
  registry.register({id:'test.scope',apiVersion:1,execution:'local',capabilities:['continue']},async()=>{runs++;return {};});
  const queue=await new JobQueue(store,registry).init();queue.active={id:'test-blocker'};
  const job=await queue.submit({projectId:p.id,projectRevision:0,pluginId:'test.scope',capability:'continue'});
  p.tracks[0].notes[0].velocity16=60000;await store.saveProject(p,0);queue.active=null;await queue.drain();
  assert.equal(queue.jobs.get(job.id).state,'failed');assert.match(queue.jobs.get(job.id).error,/edited while/);assert.equal(runs,0);
});
