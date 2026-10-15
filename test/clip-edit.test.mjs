import test from 'node:test';
import assert from 'node:assert/strict';
import { mkdtemp, rm } from 'node:fs/promises';
import { join } from 'node:path';
import { tmpdir } from 'node:os';
import { clipWindow, clipGainPoints, editClip, restoreClip, analysisWindow, validateAnalysisWindow } from '../public/clip-edit.js';
import { StudioAudio, bufferWAV } from '../public/audio.js';
import { candidateAudition } from '../public/note-view.js';
import { newProject, validateProject } from '../src/project.mjs';
import { Store } from '../src/storage.mjs';
import { exportArchive, importArchive } from '../src/archive.mjs';

function fixture(){
  const p=newProject('Clip source preservation');p.bars=1;p.tempo=60;
  Object.assign(p.tracks[0],{kind:'audio',instrument:'voice',notes:[],assetId:'a'.repeat(64)+'.wav',audio:{origin:'import',mimeType:'audio/wav',durationSeconds:3.2,byteLength:282284,originalName:'Source.wav',external:{keep:true}},externalMetadata:{namespace:'retained'}});
  p.tracks[1].notes=[{id:'original-note',pitch:60,startTick:123,durationTicks:987,velocity16:49123,expression:[]}];return p;
}
test('clip edits preserve source identity, raw metadata and other performances; originals restore exactly',()=>{
  const p=fixture(),before=structuredClone(p),id=p.tracks[0].id,next=editClip(p,id,{startTick:4001,trimStartSeconds:.8,trimEndSeconds:2.4});
  assert.deepEqual(p,before);assert.equal(next.bars,2);assert.equal(validateProject(next),next);assert.equal(next.tracks[0].assetId,p.tracks[0].assetId);assert.deepEqual(next.tracks[1],p.tracks[1]);
  assert.deepEqual(next.tracks[0].audio.editSource,{version:1,original:{}});assert.deepEqual(next.tracks[0].audio.external,{keep:true});assert.equal(next.tracks[0].audio.durationSeconds,3.2);
  const again=editClip(next,id,{trimStartSeconds:1});assert.deepEqual(again.tracks[0].audio.editSource,next.tracks[0].audio.editSource);
  assert.deepEqual(restoreClip(again,id).tracks,p.tracks);assert.equal(restoreClip(again,id).bars,2);
  p.tracks[0].audio.startTick=123;p.tracks[0].audio.trimStartSeconds=.1;p.tracks[0].audio.trimEndSeconds=3;
  assert.deepEqual(restoreClip(editClip(p,id,{startTick:500}),id).tracks,p.tracks);
});
test('clip validation rejects reversed, empty, non-finite, out-of-source and excessive placement',()=>{
  const p=fixture(),id=p.tracks[0].id;
  for(const patch of [{startTick:-1},{startTick:1.5},{startTick:null},{startTick:undefined},{trimStartSeconds:-.1},{trimStartSeconds:3.2},{trimEndSeconds:3.3},{trimStartSeconds:2,trimEndSeconds:1},{trimEndSeconds:0},{trimEndSeconds:NaN},{trimEndSeconds:Infinity},{durationSeconds:1},{startTick:128*3840-100}])assert.throws(()=>editClip(p,id,patch));
  const boundary=editClip(p,id,{startTick:128*3840-960,trimEndSeconds:1});assert.equal(boundary.bars,128);
  assert.equal(editClip(p,id,{startTick:0,trimStartSeconds:0,trimEndSeconds:3.2}),p);
  const damaged=structuredClone(p);damaged.tracks[0].audio.trimEndSeconds=null;assert.throws(()=>validateProject(damaged));
});
test('locks and invalid original clip snapshots prevent mutation',()=>{
  const p=fixture(),id=p.tracks[0].id;p.tracks[0].locked=true;
  assert.throws(()=>editClip(p,id,{startTick:1}),/Unlock/);assert.throws(()=>restoreClip(p,id),/Unlock/);
  p.tracks[0].locked=false;assert.throws(()=>editClip(p,p.tracks[1].id,{startTick:1}),/audio clip/);
  for(const original of [{startTick:-1},{trimStartSeconds:9},{trimEndSeconds:null},{durationSeconds:999},{editSource:{}}]){const broken=editClip(p,id,{startTick:1});broken.tracks[0].audio.editSource.original=original;assert.throws(()=>validateProject(broken));}
});
test('analysis windows use declared sample frames and reject stale source trims',()=>{
  const audio={durationSeconds:3.2,trimStartSeconds:.8,trimEndSeconds:2.4},window=analysisWindow({length:141120,sampleRate:44100},audio);
  assert.equal(window.startFrame,35280);assert.equal(window.endFrame,105840);assert.equal(window.startSeconds,.8);assert.equal(window.endSeconds,2.4);assert.deepEqual(validateAnalysisWindow(audio,window),window);
  assert.throws(()=>validateAnalysisWindow({...audio,trimStartSeconds:.9},window),/trim changed/);assert.throws(()=>validateAnalysisWindow(audio,{...window,endFrame:141120}));
  const shorter=analysisWindow({length:132300,sampleRate:44100},{durationSeconds:3.2});assert.equal(shorter.endSeconds,3);assert.equal(shorter.requestedEndSeconds,3.2);
  assert.throws(()=>analysisWindow({length:132300,sampleRate:44100},{...audio,trimStartSeconds:3.1,trimEndSeconds:3.2}));
});
test('analysis WAV contains only selected frames and preserves the decoded original buffers',()=>{
  const channels=[new Float32Array([.1,.2,.3,.4]),new Float32Array([-.1,0,.5,1])],before=channels.map(a=>Array.from(a)),buffer={length:4,numberOfChannels:2,sampleRate:44100,getChannelData:i=>channels[i]},wav=new DataView(bufferWAV(buffer,{startFrame:1,endFrame:3}));
  assert.equal(wav.byteLength,48);assert.equal(wav.getUint32(40,true),4);assert.equal(wav.getInt16(44,true),3277);assert.equal(wav.getInt16(46,true),13107);assert.deepEqual(channels.map(a=>Array.from(a)),before);
  assert.throws(()=>bufferWAV(buffer,{startFrame:3,endFrame:1}));
});
test('arrangement playback and audition honor source in/out and late source offsets',async t=>{
  const p=fixture();p.tracks=[p.tracks[0]];Object.assign(p.tracks[0].audio,{startTick:2880,trimStartSeconds:.25,trimEndSeconds:.75});
  const original=structuredClone(p),preview=candidateAudition(p,{tracks:p.tracks,auditionStartTick:2880});assert.equal(preview.bars,1);assert.equal(preview.tracks[0].audio.startTick,0);assert.equal(preview.tracks[0].audio.trimStartSeconds,.25);assert.deepEqual(p,original);
  const nodes=[],context={currentTime:0,createBufferSource(){const node={connect(){},disconnect(){},start(at,offset){this.startAt=at;this.offset=offset;},stop(at){this.stopAt=at;}};nodes.push(node);return node;},createGain(){return {gain:{value:0,cancelScheduledValues(){}},connect(){},disconnect(){}};}};
  const audio=new StudioAudio();audio.context=context;audio.master={};audio.ready=async()=>context;audio.loadAsset=async id=>{const buffer={duration:3.2};audio.buffers.set(id,buffer);return buffer;};t.after(()=>audio.stop());
  await audio.play(p);context.currentTime=2.98;await new Promise(r=>setTimeout(r,40));assert.equal(nodes[0].startAt,3.06);assert.equal(nodes[0].offset,.25);assert.equal(nodes[0].stopAt,3.56);
  audio.stop();nodes.length=0;context.currentTime=0;await audio.play(p);context.currentTime=3.1;await new Promise(r=>setTimeout(r,40));assert.ok(Math.abs(nodes[0].offset-.29)<1e-9);assert.equal(nodes[0].stopAt,3.56);
});
test('complete clip archive retains source bytes and original placement for restoration',async t=>{
  const dir=await mkdtemp(join(tmpdir(),'omniusong-clip-edit-'));t.after(()=>rm(dir,{recursive:true,force:true}));
  const source=await new Store(join(dir,'source')).init(),destination=await new Store(join(dir,'restored')).init(),p=fixture(),bytes=Buffer.from('complete synthetic source bytes');p.tracks[0].assetId=await source.putAsset(bytes,'wav');p.tracks[0].audio.byteLength=bytes.length;
  const edited=editClip(p,p.tracks[0].id,{startTick:4001,trimStartSeconds:.8,trimEndSeconds:2.4,fadeInSeconds:.2,fadeOutSeconds:.4}),saved=await source.saveProject(edited,null),restored=await importArchive(destination,await exportArchive(source,saved));
  assert.deepEqual(restored.project.tracks,saved.tracks);assert.deepEqual(restored.project.history,saved.history);assert.deepEqual(await destination.asset(p.tracks[0].assetId),bytes);assert.deepEqual(restoreClip(restored.project,p.tracks[0].id).tracks,p.tracks);
});
test('fades are optional, bounded by the trim, reversible, and independent of source analysis',()=>{
  const p=fixture(),id=p.tracks[0].id,original=structuredClone(p),window=analysisWindow({length:141120,sampleRate:44100},p.tracks[0].audio);
  assert.equal(clipWindow(p.tracks[0].audio).fadeInSeconds,0);assert.equal(editClip(p,id,{fadeInSeconds:0,fadeOutSeconds:0}),p);
  for(const patch of [{fadeInSeconds:-.1},{fadeOutSeconds:NaN},{fadeInSeconds:null},{fadeOutSeconds:Infinity},{fadeInSeconds:3,fadeOutSeconds:.3},{trimEndSeconds:1,fadeInSeconds:.7,fadeOutSeconds:.4}])assert.throws(()=>editClip(p,id,patch));
  const next=editClip(p,id,{fadeInSeconds:.4,fadeOutSeconds:.8});assert.equal(validateProject(next),next);assert.deepEqual(p,original);assert.deepEqual(next.tracks[1],p.tracks[1]);
  assert.deepEqual(validateAnalysisWindow(next.tracks[0].audio,window),window);assert.deepEqual(restoreClip(next,id).tracks,p.tracks);
  assert.throws(()=>editClip(next,id,{trimEndSeconds:1}),/Fades/);
  p.tracks[0].audio.fadeInSeconds=.1;p.tracks[0].audio.fadeOutSeconds=.2;
  assert.deepEqual(restoreClip(editClip(p,id,{fadeInSeconds:.3}),id).tracks,p.tracks);
  const broken=structuredClone(next);broken.tracks[0].audio.editSource.original={fadeOutSeconds:9};assert.throws(()=>validateProject(broken));
});
test('linear envelopes retain their level on a late start and fit shortened decoded or loop spans',()=>{
  const audio={durationSeconds:4,fadeInSeconds:1,fadeOutSeconds:2};
  assert.deepEqual(clipGainPoints(audio,4),[{seconds:0,level:0},{seconds:1,level:1},{seconds:2,level:1},{seconds:4,level:0}]);
  assert.deepEqual(clipGainPoints(audio,4,.25),[{seconds:.25,level:.25},{seconds:1,level:1},{seconds:2,level:1},{seconds:4,level:0}]);
  assert.deepEqual(clipGainPoints(audio,4,3),[{seconds:3,level:.5},{seconds:4,level:0}]);
  assert.deepEqual(clipGainPoints(audio,1.5),[{seconds:0,level:0},{seconds:.5,level:1},{seconds:1.5,level:0}]);
  assert.deepEqual(clipGainPoints({durationSeconds:2,fadeOutSeconds:.5},2),[{seconds:0,level:1},{seconds:1.5,level:1},{seconds:2,level:0}]);
  assert.deepEqual(clipGainPoints({durationSeconds:2,fadeInSeconds:.5},2),[{seconds:0,level:0},{seconds:.5,level:1},{seconds:2,level:1}]);
  for(const [duration,elapsed]of [[0,0],[5,0],[4,-1],[4,4],[NaN,0]])assert.throws(()=>clipGainPoints(audio,duration,elapsed));
});
test('audio scheduler applies fades in arrangement and audition without modifying the decoded source',async t=>{
  const p=fixture();p.tracks=[p.tracks[0]];p.tracks[0].gain=.6;Object.assign(p.tracks[0].audio,{startTick:2880,trimStartSeconds:.25,trimEndSeconds:2.25,fadeInSeconds:.25,fadeOutSeconds:.5});
  const original=structuredClone(p),nodes=[],buffer={duration:3.2},context={currentTime:0,createBufferSource(){const node={connect(){},disconnect(){},start(at,offset){this.startAt=at;this.offset=offset;},stop(at){this.stopAt=at;}};nodes.push(node);return node;},createGain(){const events=[],gain={value:0,cancelScheduledValues(){},setValueAtTime(value,at){events.push(['set',value,at]);},linearRampToValueAtTime(value,at){events.push(['ramp',value,at]);}};return {gain,connect(){},disconnect(){},events};}};
  const audio=new StudioAudio();audio.context=context;audio.master={};audio.ready=async()=>context;audio.loadAsset=async id=>{audio.buffers.set(id,buffer);return buffer;};t.after(()=>audio.stop());
  await audio.play(p);context.currentTime=2.98;await new Promise(r=>setTimeout(r,40));
  let voice=[...audio.voices.values()][0];assert.equal(nodes[0].startAt,3.06);assert.equal(nodes[0].stopAt,4.06);assert.equal(nodes[0].offset,.25);
  const roundedEvents=voice=>voice.gain.events.map(([kind,value,at])=>[kind,Math.round(value*1e9)/1e9,Math.round(at*1e9)/1e9]);
  assert.deepEqual(roundedEvents(voice),[['set',0,3.06],['ramp',.6,3.31],['ramp',.6,3.56],['ramp',0,4.06]]);
  audio.stop();nodes.length=0;context.currentTime=0;await audio.play(p);context.currentTime=3.1;await new Promise(r=>setTimeout(r,40));voice=[...audio.voices.values()][0];
  assert.ok(Math.abs(voice.gain.events[0][1]-.096)<1e-9);assert.equal(voice.gain.events[0][2],3.1);
  audio.stop();nodes.length=0;context.currentTime=0;await audio.play(candidateAudition(p,{tracks:p.tracks,auditionStartTick:2880}),()=>{},{mode:'audition'});voice=[...audio.voices.values()][0];
  assert.deepEqual(roundedEvents(voice),[['set',0,.06],['ramp',.6,.31],['ramp',.6,1.56],['ramp',0,2.06]]);assert.equal(nodes[0].buffer,buffer);assert.deepEqual(buffer,{duration:3.2});assert.deepEqual(p,original);
});
