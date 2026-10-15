import test from 'node:test';
import assert from 'node:assert/strict';
import { TakeCapture, audioType } from '../public/recording.js';
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
