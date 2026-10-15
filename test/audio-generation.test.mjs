import test from 'node:test';
import assert from 'node:assert/strict';
import { magentaScope, magentaCandidate } from '../src/audio-generation.mjs';
import { newProject, mergeCandidate, validateProject } from '../src/project.mjs';
import { candidateAudition } from '../public/note-view.js';
import { StudioAudio } from '../public/audio.js';

const sourceNote={id:'source',pitch:60,startTick:3700,durationTicks:5000,velocity16:51312,expression:[{type:'pitch',points:[{tick:0,value32:0x98765432}]}]};
function fixture(){const p=newProject();p.bars=2;p.tempo=104;p.tracks[0].locked=true;p.tracks[0].notes=[structuredClone(sourceNote)];return p;}
function options(p,changes={}){return {prompt:'warm keys',seed:42,startBar:2,durationSeconds:4,guideWithNotes:true,sourceTrackId:p.tracks[0].id,...changes};}
test('Magenta scope preserves the source and records the exact cropped guidance',()=>{
  const p=fixture(),original=structuredClone(p),{request,scope}=magentaScope(p,options(p));
  assert.deepEqual(p,original);assert.equal(scope.startTick,3840);assert.equal(scope.minimumBars,3);assert.equal(scope.clippedNotes,1);
  assert.deepEqual(request.notes,[{pitch:60,startTick:0,durationTicks:3840}]);assert.deepEqual(scope.guideNotes,request.notes);assert.equal(scope.sourceProjectRevision,0);
  assert.equal(magentaScope(p,options(p,{guideWithNotes:false,sourceTrackId:'missing'})).request.notes.length,0);
});
test('Magenta scope rejects unsupported duration, source, placement and note budgets',()=>{
  const p=fixture();for(const changes of [{durationSeconds:3},{seed:-1},{prompt:''},{startBar:3},{guideWithNotes:'yes'},{sourceTrackId:p.tracks[1].id}])assert.throws(()=>magentaScope(p,options(p,changes)));
  p.tracks[0].notes=[];assert.throws(()=>magentaScope(p,options(p)),/No source notes/);
  p.tracks[0].notes=Array.from({length:257},(_,i)=>({...sourceNote,id:'source'+i}));assert.throws(()=>magentaScope(p,options(p)),/256 guide notes/);
  p.bars=128;assert.throws(()=>magentaScope(p,options(p,{startBar:128,guideWithNotes:false,durationSeconds:8})),/128 bars/);
});
test('fractional tempo boundaries never send a guide note beyond the worker duration',()=>{
  const p=fixture();p.tempo=103.33;p.tracks[0].notes=[{...sourceNote,startTick:3840,durationTicks:20000}];p.bars=4;
  const {request}=magentaScope(p,options(p));assert.ok(request.notes[0].durationTicks/960*60/p.tempo<=4);
});
test('accepted audio keeps its placement while immediate audition is a separate copy',()=>{
  const p=fixture(),{scope}=magentaScope(p,options(p)),result={model:'fixture',modelSha256:'graph-sha',assetManifestSha256:'manifest-sha',durationSeconds:4,audioBytes:768044,seed:42};
  const candidate=magentaCandidate(p,scope,result,'a'.repeat(64)+'.wav'),before=structuredClone(candidate),preview=candidateAudition(p,candidate),accepted=mergeCandidate(p,candidate,{ai:true});
  assert.equal(preview.tracks[0].audio.startTick,0);assert.equal(preview.bars,2);assert.deepEqual(candidate,before);
  assert.equal(accepted.bars,3);assert.equal(accepted.tracks.at(-1).audio.startTick,3840);assert.deepEqual(accepted.tracks[0],p.tracks[0]);
  assert.equal(candidate.modelSha256,'graph-sha');assert.equal(accepted.tracks.at(-1).generation.assetManifestSha256,'manifest-sha');
  accepted.tracks.at(-1).audio.startTick=-1;assert.throws(()=>validateProject(accepted),/audio start/);
});
test('audio playback schedules the clip at its saved tick and stops at the loop boundary',async t=>{
  const p=fixture();p.tempo=60;p.bars=1;const clip={...p.tracks[0],kind:'audio',assetId:'fixture',muted:false,notes:[],audio:{startTick:2880,durationSeconds:2}};p.tracks=[clip];
  const nodes=[],context={currentTime:0,createBufferSource(){const node={connect(){},disconnect(){},start(at,offset){this.startAt=at;this.offset=offset;},stop(at){this.stopAt=at;}};nodes.push(node);return node;},createGain(){return {gain:{value:0,cancelScheduledValues(){}},connect(){},disconnect(){}};}};
  const audio=new StudioAudio();audio.context=context;audio.master={};audio.ready=async()=>context;audio.loadAsset=async id=>{const buffer={duration:2};audio.buffers.set(id,buffer);return buffer;};t.after(()=>audio.stop());
  await audio.play(p);assert.equal(nodes.length,0);context.currentTime=2.98;await new Promise(r=>setTimeout(r,40));
  assert.equal(nodes.length,1);assert.equal(nodes[0].startAt,3.06);assert.ok(Math.abs(nodes[0].stopAt-4.06)<1e-9);
  audio.stop();nodes.length=0;context.currentTime=0;await audio.play(p);context.currentTime=3.1;await new Promise(r=>setTimeout(r,40));
  assert.equal(nodes[0].startAt,3.1);assert.ok(Math.abs(nodes[0].offset-.04)<1e-9);assert.ok(Math.abs(nodes[0].stopAt-4.06)<1e-9);
});
