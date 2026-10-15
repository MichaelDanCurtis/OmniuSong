import test from 'node:test';
import assert from 'node:assert/strict';
import { mkdtemp, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { basicPitchAvailable } from '../src/worker.mjs';
import { encodeWAV } from '../src/wav.mjs';
import { newProject, validateProject, mergeCandidate } from '../src/project.mjs';
import { exportUMPTrace } from '../src/midi.mjs';
import { Store } from '../src/storage.mjs';
import { createPlugins } from '../src/plugins.mjs';
import { analysisWindow } from '../public/clip-edit.js';
import { bufferWAV } from '../public/audio.js';

test('installed Basic Pitch performs real ONNX inference and preserves pitch estimates', {skip:!await basicPitchAvailable(),timeout:60000},async t=>{
  const dir=await mkdtemp(join(tmpdir(),'omniusong-model-'));t.after(()=>rm(dir,{recursive:true,force:true}));
  const sr=44100,expected=[60,64,67,72],samples=new Float32Array(sr*3.2);
  for(let n=0;n<expected.length;n++){
    const f=440*2**((expected[n]-69)/12);
    for(let i=0;i<sr*.65;i++){const time=i/sr,env=Math.min(time/.02,1)*Math.min((.65-time)/.05,1);samples[Math.round(n*.8*sr)+i]=.3*env*(Math.sin(2*Math.PI*f*time)+.3*Math.sin(4*Math.PI*f*time));}
  }
  const store=await new Store(dir).init(),bytes=encodeWAV(samples,sr),assetId=await store.putAsset(bytes),source=newProject();
  Object.assign(source.tracks[0],{kind:'audio',assetId,audio:{origin:'import',mimeType:'audio/wav',durationSeconds:3.2,byteLength:bytes.length,startTick:3840}});
  const original=structuredClone(source),result=await createPlugins(store).run('spotify.basic-pitch',{project:source,input:{assetId,sourceAssetId:assetId,sourceTrackId:source.tracks[0].id},capability:'transcribe',signal:new AbortController().signal}),notes=result.tracks[0].notes;
  assert.deepEqual(source,original);assert.deepEqual(notes.map(n=>n.pitch),expected);assert.ok(notes.every(n=>n.startTick>=3840));
  assert.equal(result.modelSha256,'2c3c1d144bfa61ad236e92e169c13535c880469a12a047d4e73451f2c059a0ec');
  assert.ok(notes.every(n=>n.sourceEstimate.velocityIsEstimated));
  assert.ok(notes.some(n=>n.expression[0]?.points.length>0));
  const p=mergeCandidate(source,result,{ai:true});validateProject(p);assert.equal(p.tracks.at(-1).transcription.placementStartTick,3840);
  assert.ok(exportUMPTrace(p).packets.length>expected.length*2);
  const cropped=structuredClone(source);Object.assign(cropped.tracks[0].audio,{trimStartSeconds:.8,trimEndSeconds:2.4});
  const window=analysisWindow({sampleRate:sr,length:samples.length},cropped.tracks[0].audio),shortBytes=Buffer.from(bufferWAV({sampleRate:sr,length:samples.length,numberOfChannels:1,getChannelData:()=>samples},window)),shortId=await store.putAsset(shortBytes);
  const cropBefore=structuredClone(cropped),input={assetId:shortId,sourceAssetId:assetId,sourceTrackId:cropped.tracks[0].id,analysisWindow:window};
  const trimmed=await createPlugins(store).run('spotify.basic-pitch',{project:cropped,input,capability:'transcribe',signal:new AbortController().signal});
  assert.deepEqual(cropped,cropBefore);assert.deepEqual(trimmed.tracks[0].notes.map(n=>n.pitch),[64,67]);assert.ok(trimmed.tracks[0].notes.every(n=>n.startTick>=3840));
  assert.equal(trimmed.tracks[0].sourceAssetId,assetId);assert.deepEqual(trimmed.tracks[0].transcription.sourceWindow,window);assert.equal(trimmed.tracks[0].transcription.sourceDurationSeconds,1.6);assert.equal(trimmed.tracks[0].transcription.estimateTimeOrigin,'analysis-window-start');
  await assert.rejects(createPlugins(store).run('spotify.basic-pitch',{project:cropped,input:{...input,analysisWindow:{...window,startFrame:0}},capability:'transcribe',signal:new AbortController().signal}),/trim changed/);
  await assert.rejects(createPlugins(store).run('spotify.basic-pitch',{project:cropped,input:{...input,assetId},capability:'transcribe',signal:new AbortController().signal}),/analysis WAV/);
  const wrongRate=analysisWindow({sampleRate:96000,length:307200},cropped.tracks[0].audio);
  await assert.rejects(createPlugins(store).run('spotify.basic-pitch',{project:cropped,input:{...input,analysisWindow:wrongRate},capability:'transcribe',signal:new AbortController().signal}),/analysis WAV/);
  await assert.rejects(createPlugins(store).run('spotify.basic-pitch',{project:cropped,input:{assetId:shortId,sourceAssetId:assetId,sourceTrackId:cropped.tracks[0].id},capability:'transcribe',signal:new AbortController().signal}),/trimmed clip/);
});
