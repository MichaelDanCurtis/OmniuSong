import test from 'node:test';
import assert from 'node:assert/strict';
import { mkdtemp, rm } from 'node:fs/promises';
import { join } from 'node:path';
import { tmpdir } from 'node:os';
import { transportSettings, transportClock } from '../public/transport.js';
import { StudioAudio } from '../public/audio.js';
import { candidateAudition } from '../public/note-view.js';
import { newProject, validateProject } from '../src/project.mjs';
import { Store } from '../src/storage.mjs';
import { exportArchive, importArchive } from '../src/archive.mjs';

const delay=()=>new Promise(r=>setTimeout(r,40));
function fixture(){const p=newProject('Count-in fixture');p.tempo=120;p.bars=2;p.transport={version:1,metronome:true,countInBars:1,clickLevel:.35,external:{keep:true}};p.tracks[0].notes=[{id:'original',pitch:60,startTick:0,durationTicks:480,velocity16:49123,expression:[]}];return p;}
function engine(t){
  const clicks=[],music=[],states=[],context={currentTime:0,
    createOscillator(){const node={frequency:{setValueAtTime(value){node.frequencyValue=value;}},connect(){},disconnect(){},start(at){this.startAt=at;},stop(at){(this.stops??=[]).push(at);}};clicks.push(node);return node;},
    createGain(){return {gain:{value:0,setValueAtTime(){},linearRampToValueAtTime(){},exponentialRampToValueAtTime(){},cancelScheduledValues(){}},connect(){},disconnect(){}};}};
  const audio=new StudioAudio();audio.context=context;audio.master={};audio.ready=async()=>context;audio.voice=(...args)=>music.push(args);t.after(()=>audio.stop());return {audio,context,clicks,music,states,position:(_tick,state)=>states.push(state)};
}
test('transport defaults and validated settings preserve external metadata',()=>{
  assert.deepEqual(transportSettings(),{version:1,metronome:false,countInBars:0,clickLevel:.35});
  const p=fixture();assert.deepEqual(transportSettings(p.transport).external,{keep:true});assert.equal(validateProject(p),p);
  for(const value of [null,{},[],{...p.transport,version:2},{...p.transport,metronome:1},{...p.transport,countInBars:3},{...p.transport,countInBars:-1},{...p.transport,clickLevel:NaN},{...p.transport,clickLevel:Infinity},{...p.transport,clickLevel:-.1},{...p.transport,clickLevel:1.1}]){p.transport=value;assert.throws(()=>validateProject(p),/transport/);}
});
test('one audio clock separates the count-in from song ticks at the exact boundary',()=>{
  const p=fixture(),clock=transportClock(p,.06,2);assert.equal(clock.countInBeats,8);assert.equal(clock.startTime,4.06);
  assert.equal(clock.position(4.05999).phase,'count-in');assert.equal(clock.position(4.05999).tick,0);assert.equal(clock.position(4.05999).countInBeat,8);
  assert.equal(clock.position(4.06).phase,'playing');assert.equal(clock.position(4.06).tick,0);assert.equal(clock.position(4.56).beat,2);assert.ok(Math.abs(clock.position(5.06).tick-1920)<1e-9);
  assert.ok(Math.abs(clock.position(8.06).tick)<1e-9);assert.equal(transportClock(p,.06,0).position(0).phase,'starting');
  const fractional=transportClock({...p,tempo:103.33},10,1);assert.equal(fractional.position(fractional.startTime).tick,0);
});
test('count-in clicks precede music and recording readiness begins only at song start',async t=>{
  const e=engine(t),p=fixture(),before=structuredClone(p);await e.audio.play(p,e.position);assert.equal(e.audio.recordingReady(),false);assert.equal(e.music.length,0);assert.equal(e.clicks[0].startAt,.06);assert.equal(e.clicks[0].frequencyValue,1760);
  for(const now of [.48,.98,1.48]){e.context.currentTime=now;await delay();}
  assert.deepEqual(e.clicks.map(n=>n.startAt),[.06,.56,1.06,1.56]);assert.ok(e.clicks.slice(1).every(n=>n.frequencyValue===1100));
  e.context.currentTime=1.99;await delay();assert.equal(e.music[0][3],2.06);assert.equal(e.clicks.at(-1).startAt,2.06);assert.equal(e.clicks.at(-1).frequencyValue,1760);assert.equal(e.audio.recordingReady(),false);
  e.context.currentTime=2.06;assert.equal(e.audio.recordingReady(),true);assert.equal(e.audio.tick(),0);assert.deepEqual(p,before);
});
test('stop cancels count-in sources and never starts the song later',async t=>{
  const e=engine(t);await e.audio.play(fixture(),e.position);e.audio.stop();e.context.currentTime=20;await delay();assert.equal(e.music.length,0);assert.equal(e.audio.recordingReady(),false);assert.equal(e.audio.playing,false);assert.equal(e.audio.voices.size,0);assert.equal(e.audio.phase,'stopped');assert.equal(e.clicks[0].stops.at(-1),undefined);
});
test('pending startup can be canceled and failures leave a stopped transport',async t=>{
  const e=engine(t);let ready;e.audio.ready=()=>new Promise(r=>ready=r);const pending=e.audio.play(fixture(),e.position);assert.equal(e.audio.playing,true);assert.equal(e.audio.position().phase,'loading');assert.equal(e.audio.recordingReady(),false);e.audio.stop();ready(e.context);assert.equal(await pending,false);assert.equal(e.clicks.length,0);
  const p=fixture();p.tracks[0].assetId='fixture';let fail;e.audio.ready=async()=>e.context;e.audio.loadAsset=()=>new Promise((_r,reject)=>fail=reject);const loading=e.audio.play(p,e.position);await Promise.resolve();assert.equal(e.audio.position().phase,'loading');e.audio.stop();fail(new Error('late decode failure'));assert.equal(await loading,false);assert.equal(e.audio.position().phase,'stopped');
  e.audio.loadAsset=async()=>{throw new Error('decode failed');};await assert.rejects(e.audio.play(p,e.position),/decode failed/);assert.equal(e.audio.playing,false);assert.equal(e.audio.phase,'stopped');
});
test('audition skips project count-in and metronome and cannot record input',async t=>{
  const e=engine(t),p=fixture(),before=structuredClone(p),preview=candidateAudition(p,{tracks:p.tracks,auditionStartTick:0});assert.equal(preview.transport.countInBars,0);assert.equal(preview.transport.metronome,false);preview.transport.external.keep=false;assert.deepEqual(p,before);
  await e.audio.play(p,e.position,{mode:'audition'});assert.equal(e.audio.startTime,.06);assert.equal(e.clicks.length,0);e.context.currentTime=1;e.audio.setMetronome(true,.5);await delay();assert.equal(e.clicks.length,0);assert.equal(e.audio.recordingReady(),false);assert.equal(e.audio.position().mode,'audition');assert.deepEqual(p,before);
});
test('live metronome changes preserve the clock and stalled scheduling skips a backlog',async t=>{
  const e=engine(t),p=fixture();p.transport.countInBars=0;await e.audio.play(p,e.position);const start=e.audio.startTime;e.context.currentTime=.2;e.audio.setMetronome(false,.5);assert.equal(e.audio.startTime,start);assert.equal(e.audio.recordingReady(),true);assert.equal(e.audio.voices.size,0);const prior=e.clicks.length;e.context.currentTime=.48;await delay();assert.equal(e.clicks.length,prior);
  e.audio.setMetronome(true,.5);await delay();assert.equal(e.clicks.at(-1).startAt,.56);const musicBefore=e.music.length,clickBefore=e.clicks.length;e.context.currentTime=10000;await delay();assert.ok(e.music.length-musicBefore<=1);assert.ok(e.clicks.length-clickBefore<=1);assert.equal(e.audio.recordingReady(),true);
});
test('count-in preferences and original notes survive complete project archives',async t=>{
  const dir=await mkdtemp(join(tmpdir(),'omniusong-transport-'));t.after(()=>rm(dir,{recursive:true,force:true}));const source=await new Store(join(dir,'source')).init(),destination=await new Store(join(dir,'destination')).init(),p=await source.saveProject(fixture(),null),restored=await importArchive(destination,await exportArchive(source,p));assert.deepEqual(restored.project.transport,p.transport);assert.deepEqual(restored.project.tracks,p.tracks);
});
