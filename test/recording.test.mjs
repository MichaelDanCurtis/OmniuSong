import test from 'node:test';
import assert from 'node:assert/strict';
import { TakeCapture, audioType, recordingTiming, recordingPlacement, takePlacement } from '../public/recording.js';
import { transportClock } from '../public/transport.js';
import { newProject, validateProject } from '../src/project.mjs';
import { editClip, restoreClip } from '../public/clip-edit.js';
import { Store } from '../src/storage.mjs';
import { exportArchive, importArchive } from '../src/archive.mjs';
import { mkdtemp, rm } from 'node:fs/promises';
import { join } from 'node:path';
import { tmpdir } from 'node:os';
const turn=()=>new Promise(resolve=>setImmediate(resolve));
const deferred=()=>{let resolve,reject;const promise=new Promise((a,b)=>{resolve=a;reject=b;});return {promise,resolve,reject};};
function fixture(options={}){
  const states=[],errors=[],saved=[],tracks=[{stops:0,stop(){this.stops++;},addEventListener(type,fn){this[type]=fn;}}];
  const stream={getTracks:()=>tracks};
  const recorder={state:'inactive',mimeType:'audio/webm',start(){this.state='recording';},stop(){this.state='inactive';queueMicrotask(()=>{this.ondataavailable({data:new Blob(['final audio'],{type:this.mimeType})});this.onstop();});}};
  const capture=new TakeCapture({getStream:async()=>stream,createRecorder:()=>recorder,persist:async(blob,context)=>saved.push({blob,context:{...context}}),onState:s=>states.push(s.phase),onError:e=>errors.push(e.message),setTimer:()=>1,clearTimer:()=>{},now:()=>1000,...options});
  return {capture,tracks,recorder,stream,states,errors,saved};
}
test('permission is busy immediately; cancellation stops a late stream and never records',async()=>{
  const permission=deferred(),f=fixture({getStream:()=>permission.promise}),starting=f.capture.start({projectId:'original'});
  assert.equal(f.capture.phase,'requesting');assert.equal(f.capture.busy,true);f.capture.stop();permission.resolve(f.stream);await starting;
  assert.equal(f.tracks[0].stops,1);assert.equal(f.capture.phase,'idle');assert.equal(f.saved.length,0);
});
test('a canceled permission result cannot reset a newer recording',async()=>{
  const permission=deferred(),f=fixture({getStream:()=>permission.promise}),first=f.capture.start({projectId:'first'});f.capture.stop();
  f.capture.getStream=async()=>f.stream;await f.capture.start({projectId:'second'});permission.reject(new Error('old denial'));await first;
  assert.equal(f.capture.phase,'recording');assert.equal(f.capture.take.context.projectId,'second');assert.equal(f.errors.length,0);f.capture.stop();await turn();
});
test('denied permission and recorder construction errors clean up and allow another take',async()=>{
  const f=fixture({getStream:async()=>{throw new Error('permission denied');}});await f.capture.start({projectId:'a'});assert.equal(f.capture.busy,false);assert.deepEqual(f.errors,['permission denied']);
  f.capture.getStream=async()=>f.stream;f.capture.createRecorder=()=>{throw new Error('unsupported recorder');};await f.capture.start({projectId:'a'});assert.equal(f.tracks[0].stops,1);assert.equal(f.capture.busy,false);
});
test('finish waits for final audio and persistence; repeated stop cannot duplicate a take',async()=>{
  const persistence=deferred(),f=fixture({persist:()=>persistence.promise});await f.capture.start({projectId:'a'});f.capture.stop();f.capture.stop();await turn();
  assert.equal(f.capture.phase,'saving');assert.equal(await f.capture.blob.text(),'final audio');assert.equal(f.tracks[0].stops,1);await assert.rejects(f.capture.start({projectId:'b'}),/Finish/);
  persistence.resolve();await turn();assert.equal(f.capture.busy,false);assert.deepEqual(f.states,['requesting','recording','stopping','saving','idle']);
});
test('failed persistence retains exact original bytes and context for retry',async()=>{
  let attempts=0;const seen=[],f=fixture({persist:async(blob,context)=>{seen.push({blob,projectId:context.projectId});if(++attempts===1)throw new Error('disk unavailable');}}),context={projectId:'a'};
  await f.capture.start(context);context.projectId='b';f.capture.stop();await turn();assert.equal(f.capture.phase,'failed');assert.equal(f.capture.busy,true);assert.equal(await f.capture.blob.text(),'final audio');
  await f.capture.retry();assert.equal(f.capture.phase,'idle');assert.equal(seen[0].blob,seen[1].blob);assert.equal(seen[1].projectId,'a');
});
test('recorder error preserves the final data event even when already inactive',async()=>{
  const f=fixture();await f.capture.start({projectId:'a'});f.recorder.state='inactive';f.recorder.onerror({error:new Error('device lost')});assert.equal(f.capture.phase,'stopping');
  f.recorder.ondataavailable({data:new Blob(['last fragment'])});f.recorder.onstop();await turn();
  assert.equal(await f.saved[0].blob.text(),'last fragment');assert.equal(f.saved[0].context.interruption,'device lost');assert.equal(f.capture.phase,'idle');
});
test('device disconnect and duration limit finalize existing bytes and release the microphone',async()=>{
  let limit;const f=fixture({setTimer:fn=>{limit=fn;return 1;}});await f.capture.start({projectId:'a'});const oldLimit=limit;f.tracks[0].ended();await turn();assert.equal(f.saved.length,1);assert.match(f.saved[0].context.interruption,/disconnected/);
  await f.capture.start({projectId:'a'});oldLimit();assert.equal(f.capture.phase,'recording');limit();await turn();assert.equal(f.saved.length,2);assert.equal(f.capture.busy,false);
});
test('imports use the same protected save lifecycle and retain the original on failure',async()=>{
  const f=fixture({persist:async()=>{throw new Error('format unavailable');}}),blob=new Blob(['original FLAC'],{type:'audio/flac'});await f.capture.import(blob,{projectId:'a',originalName:'phrase.flac'});
  assert.equal(f.capture.phase,'failed');assert.equal(f.capture.blob,blob);assert.equal(f.capture.take.context.origin,'import');f.capture.cancelImport();assert.equal(f.capture.busy,false);
  await assert.rejects(f.capture.import(new Blob([]),{}),/Choose audio/);
});
test('MIME inference accepts common browser aliases and rejects unrelated files',()=>{
  assert.equal(audioType('voice.wav','audio/x-wav'),'audio/wav');assert.equal(audioType('Voice.M4A',''),'audio/mp4');assert.equal(audioType('voice.mp3','application/octet-stream'),'audio/mpeg');assert.equal(audioType('','audio/webm;codecs=opus'),'audio/webm');assert.throws(()=>audioType('image.png','image/png'),/Choose WAV/);
});
test('song preparation follows permission; recorder timing is sampled afterward and originals survive retry',async()=>{
  const permission=deferred(),prepared=deferred(),order=[],clockProject={tempo:120,ppq:960,bars:2},clock=transportClock(clockProject,10,1);let audioTime=0,aborts=0,attempts=0;
  const f=fixture({getStream:()=>permission.promise,prepare:async(context,signal)=>{order.push('prepare');assert.equal(context.withSong,true);signal.addEventListener('abort',()=>aborts++);await prepared.promise;},atStart:()=>{order.push('timing');return {recordingTiming:recordingTiming(clockProject,clock,audioTime)};},persist:async(blob,context)=>{f.saved.push({blob,context:structuredClone(context)});if(++attempts===1)throw new Error('disk full');}});
  const start=f.capture.start({projectId:'a',withSong:true,external:{keep:true}});assert.equal(f.capture.phase,'requesting');assert.deepEqual(order,[]);
  permission.resolve(f.stream);await turn();assert.equal(f.capture.phase,'preparing');assert.equal(f.recorder.state,'inactive');assert.deepEqual(order,['prepare']);
  audioTime=10.1;prepared.resolve();await start;assert.equal(f.capture.phase,'recording');assert.deepEqual(order,['prepare','timing']);assert.equal(f.capture.take.context.recordingTiming.captureAudioTimeSeconds,10.1);
  f.capture.take.startedAt=-3000;f.capture.stop();await turn();assert.equal(f.capture.phase,'failed');assert.equal(aborts,1);assert.equal(f.tracks[0].stops,1);assert.equal(await f.capture.blob.text(),'final audio');
  await f.capture.retry();assert.equal(aborts,1);assert.equal(f.saved[0].blob,f.saved[1].blob);assert.deepEqual(f.saved[0].context,f.saved[1].context);assert.equal(f.saved[0].context.durationSeconds,4);
});
test('canceling pending preparation releases its stream and cannot disturb a newer take',async()=>{
  const ready=deferred();let canceled=0;const f=fixture({prepare:async(_context,signal)=>{signal.addEventListener('abort',()=>canceled++);await ready.promise;}}),first=f.capture.start({projectId:'first'});
  await turn();assert.equal(f.capture.phase,'preparing');f.capture.stop();assert.equal(canceled,1);assert.equal(f.tracks[0].stops,1);assert.equal(f.capture.phase,'idle');
  f.capture.prepare=null;await f.capture.start({projectId:'second'});assert.equal(f.capture.phase,'recording');ready.resolve();await first;assert.equal(f.capture.phase,'recording');assert.equal(f.capture.take.context.projectId,'second');assert.equal(f.tracks[0].stops,1);f.capture.stop();await turn();
});
test('preparation and recorder-start failures cancel the backing song and release the microphone',async()=>{
  for(const kind of ['prepare','start','timing']){let canceled=0;const f=fixture({prepare:async(_context,signal)=>{signal.addEventListener('abort',()=>canceled++);if(kind==='prepare')throw new Error('load failed');},atStart:()=>{if(kind==='timing')throw new Error('clock missing');return {};}});if(kind==='start')f.recorder.start=()=>{throw new Error('encoder unavailable');};await f.capture.start({projectId:'a'});assert.equal(canceled,1);assert.equal(f.capture.phase,'idle');assert.equal(f.tracks[0].stops,1);assert.equal(f.saved.length,0);assert.equal(f.errors.length,1);}
});
test('recording estimates trim preroll or place late starts without wrapping at the loop',()=>{
  const p={tempo:120,ppq:960,bars:2},clock=transportClock(p,10,1),early=recordingTiming(p,clock,10.25);
  assert.deepEqual(recordingPlacement(early,5),{status:'estimated',startTick:0,trimStartSeconds:1.75});
  const late=recordingTiming(p,clock,12.125);assert.deepEqual(recordingPlacement(late,2),{status:'estimated',startTick:240,trimStartSeconds:0});
  const afterLoop=recordingTiming(p,clock,16.125);assert.equal(recordingPlacement(afterLoop,1).startTick,7920);
  const fractional={...p,tempo:103.33},fractionalClock=transportClock(fractional,10,2),timing=recordingTiming(fractional,fractionalClock,10.1);assert.equal(recordingPlacement(timing,10).trimStartSeconds,fractionalClock.startTime-10.1);
  assert.equal(early.calibration,'unmeasured');assert.equal(early.method,'browser-recorder-start-call');assert.throws(()=>recordingTiming(p,null,0),/did not start/);
});
test('short count-in takes and out-of-timeline estimates retain the full source for review',()=>{
  const p={tempo:120,ppq:960,bars:2},clock=transportClock(p,10,1),early=recordingTiming(p,clock,10);
  assert.deepEqual(recordingPlacement(early,.5),{status:'ended-before-song',startTick:0,trimStartSeconds:0});assert.equal(recordingPlacement(early,2).status,'ended-before-song');
  assert.deepEqual(recordingPlacement(recordingTiming(p,clock,1000),3),{status:'outside-timeline',startTick:0,trimStartSeconds:0});
  assert.deepEqual(takePlacement({startTick:7680}),{startTick:7680});for(const startTick of [-1,1.5,128*3840,NaN])assert.throws(()=>takePlacement({startTick}));
  for(const patch of [{version:2},{calibration:'measured'},{captureAudioTimeSeconds:NaN},{tempo:0},{loopTicks:1},{ppq:480}])assert.throws(()=>recordingPlacement({...early,...patch},4),/timing estimate/);
});
test('recording estimates and complete originals survive clip edits and project archives',async t=>{
  const dir=await mkdtemp(join(tmpdir(),'omniusong-recording-clock-'));t.after(()=>rm(dir,{recursive:true,force:true}));const source=await new Store(join(dir,'source')).init(),destination=await new Store(join(dir,'destination')).init(),p=newProject('Simulated microphone timing');p.tempo=120;
  const bytes=Buffer.from('complete original including preroll'),assetId=await source.putAsset(bytes,'webm'),timing=recordingTiming(p,transportClock(p,10,1),10.25),placement=takePlacement({recordingTiming:timing,durationSeconds:5});
  p.tracks.push({id:'take',name:'Simulated take',kind:'audio',instrument:'voice',gain:.7,muted:false,locked:false,notes:[],assetId,audio:{origin:'microphone',mimeType:'audio/webm',durationSeconds:5,byteLength:bytes.length,...placement,external:{keep:true}}});
  const original=structuredClone(p.tracks.at(-1)),edited=editClip(p,'take',{startTick:100,trimStartSeconds:2});assert.deepEqual(restoreClip(edited,'take').tracks.at(-1),original);
  const saved=await source.saveProject(edited,null),restored=await importArchive(destination,await exportArchive(source,saved));assert.deepEqual(restored.project.tracks,saved.tracks);assert.deepEqual(await destination.asset(assetId),bytes);assert.deepEqual(restoreClip(restored.project,'take').tracks.at(-1),original);
  const broken=structuredClone(saved);broken.tracks.at(-1).audio.recordingTiming.status='calibrated';assert.throws(()=>validateProject(broken),/timing status/);
});
