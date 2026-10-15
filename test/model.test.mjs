import test from 'node:test';
import assert from 'node:assert/strict';
import { mkdtemp, writeFile, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { basicPitchAvailable, runBasicPitch } from '../src/worker.mjs';
import { encodeWAV } from '../src/wav.mjs';
import { newProject, validateProject } from '../src/project.mjs';
import { exportUMPTrace } from '../src/midi.mjs';

test('installed Basic Pitch performs real ONNX inference and preserves pitch estimates', {skip:!await basicPitchAvailable(),timeout:60000},async t=>{
  const dir=await mkdtemp(join(tmpdir(),'omniusong-model-'));t.after(()=>rm(dir,{recursive:true,force:true}));
  const sr=44100,expected=[60,64,67,72],samples=new Float32Array(sr*3.2);
  for(let n=0;n<expected.length;n++){
    const f=440*2**((expected[n]-69)/12);
    for(let i=0;i<sr*.65;i++){const time=i/sr,env=Math.min(time/.02,1)*Math.min((.65-time)/.05,1);samples[Math.round(n*.8*sr)+i]=.3*env*(Math.sin(2*Math.PI*f*time)+.3*Math.sin(4*Math.PI*f*time));}
  }
  const path=join(dir,'phrase.wav');await writeFile(path,encodeWAV(samples,sr));
  const result=await runBasicPitch(path,104,new AbortController().signal);
  assert.deepEqual(result.notes.map(n=>n.pitch),expected);
  assert.equal(result.modelSha256,'2c3c1d144bfa61ad236e92e169c13535c880469a12a047d4e73451f2c059a0ec');
  assert.ok(result.notes.every(n=>n.sourceEstimate.velocityIsEstimated));
  assert.ok(result.notes.some(n=>n.expression[0]?.points.length>0));
  const p=newProject();p.tracks[0].notes=result.notes;validateProject(p);
  assert.ok(exportUMPTrace(p).packets.length>expected.length*2);
});
