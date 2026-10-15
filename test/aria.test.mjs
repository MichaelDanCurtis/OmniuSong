import test from 'node:test';
import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import { ariaInstalled, runAria } from '../src/worker.mjs';

const installed=await ariaInstalled();
test('qualified Aria performs actual MLX continuation and preserves the source request',{skip:!installed,timeout:120000},async()=>{
  const request=JSON.parse(await readFile(new URL('../docs/verification/aria-prompt.json',import.meta.url),'utf8')),original=structuredClone(request);
  const result=await runAria(request,new AbortController().signal);
  assert.deepEqual(request,original);assert.equal(result.backend,'mlx');assert.equal(result.generatedTokens,64);
  assert.equal(result.decodedPromptNotes,request.notes.length);assert.ok(result.notes.length>=4);assert.equal(result.notes[0].startTick,0);
  assert.ok(result.notes.every(n=>n.modelEstimate.velocity7>0&&n.modelEstimate.startMilliseconds>=result.firstGeneratedOnsetMilliseconds));
  assert.equal(result.modelRevision,'5a1ef36c5007ee10fbe57d61d96725a7244bd2b2');
});
test('Aria rejects an invalid request and honors cancellation before inference',{skip:!installed,timeout:120000},async()=>{
  await assert.rejects(runAria({tempo:104,seed:0,maxNewTokens:257,notes:[]},new AbortController().signal),/Invalid seed or token limit/);
  const controller=new AbortController();controller.abort(new Error('Canceled fixture'));
  await assert.rejects(runAria({},controller.signal),/Canceled fixture/);
});
