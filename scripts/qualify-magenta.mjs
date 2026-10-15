import { readFile, writeFile } from 'node:fs/promises';
import assert from 'node:assert/strict';
import { runMagenta, magentaManifest } from '../src/magenta.mjs';

const fixture=JSON.parse(await readFile(new URL('../docs/verification/magenta-prompt.json',import.meta.url),'utf8'));
const receipts=[];
for(const [name,request]of [['style',{...fixture,notes:[]}],['notes',fixture]]){
  const original=structuredClone(request),{result,audio}=await runMagenta(request,new AbortController().signal);
  assert.deepEqual(request,original);await writeFile(new URL(`../docs/verification/magenta-${name}.wav`,import.meta.url),audio);
  receipts.push({name,...result});
}
assert.notEqual(receipts[0].audioSha256,receipts[1].audioSha256,'Note conditioning should change this fixture');
const receipt={qualifiedAt:new Date().toISOString(),manifest:magentaManifest,sourcePreserved:true,noteConditioningChangesOutput:true,
  scope:'Two four-second offline synthetic fixtures. No listening-quality, exact note adherence, or live audio/MIDI latency claim.',results:receipts};
await writeFile(new URL('../docs/verification/magenta.json',import.meta.url),JSON.stringify(receipt,null,2)+'\n');
process.stdout.write(JSON.stringify(receipt.results.map(r=>({name:r.name,seconds:r.durationSeconds,generationSeconds:r.generationSeconds,totalSeconds:r.elapsedSeconds,rms:r.rmsAmplitude,peakMLXBytes:r.peakMLXBytes})),null,2)+'\n');
