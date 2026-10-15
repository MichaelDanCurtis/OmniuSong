import { readFile, writeFile } from 'node:fs/promises';
import { createHash } from 'node:crypto';
import { spawnSync } from 'node:child_process';
import { runAria, ariaPython } from '../src/worker.mjs';
import { newProject, makeTrack, validateProject } from '../src/project.mjs';
import { exportSMF, exportUMPTrace } from '../src/midi.mjs';
import { renderProject } from '../src/wav.mjs';

const directory=new URL('../docs/verification/',import.meta.url);
const prompt=JSON.parse(await readFile(new URL('aria-prompt.json',directory),'utf8'));
const result=await runAria(prompt,new AbortController().signal);
const source=newProject('Aria piano · local model preview');
const promptNotes=prompt.notes.map((note,i)=>({...note,id:'prompt-'+i,expression:[]}));
const answerOffset=Math.max(...promptNotes.map(n=>n.startTick+n.durationTicks))+240;
const answer=result.notes.map(note=>({...note,startTick:note.startTick+answerOffset}));
source.tempo=prompt.tempo;source.tracks=[makeTrack('Procedural prompt','keys',promptNotes),makeTrack('AI answer · Aria MLX','keys',answer)];
source.history=[{id:'aria-qualification',action:'qualification-preview',provenance:{ai:true,model:result.model,modelSha256:result.modelSha256,promptOrigin:'procedural qualification phrase'}}];
source.tracks[0].locked=true;source.tracks[1].provenance={ai:true,model:result.model,modelRevision:result.modelRevision,modelSha256:result.modelSha256,sourceRevision:result.sourceRevision};
source.bars=Math.ceil(Math.max(...answer.map(n=>n.startTick+n.durationTicks))/3840);
validateProject(source);
const midi=exportSMF(source).bytes,wav=renderProject(source);
const checked=spawnSync(ariaPython,['-c',"import io,json,sys,mido,importlib.metadata; f=mido.MidiFile(file=io.BytesIO(sys.stdin.buffer.read())); print(json.dumps({'reader':'mido '+importlib.metadata.version('mido'),'tracks':len(f.tracks),'noteOns':sum(m.type=='note_on' and m.velocity>0 for t in f.tracks for m in t),'ticksPerBeat':f.ticks_per_beat}))"],{input:midi});
if(checked.status!==0)throw new Error('Independent MIDI 1 preview validation failed');
const independentMIDI1Read=JSON.parse(checked.stdout.toString());
if(independentMIDI1Read.noteOns!==promptNotes.length+answer.length||independentMIDI1Read.ticksPerBeat!==960)throw new Error('MIDI 1 reader found a different preview performance');
const hash=bytes=>createHash('sha256').update(bytes).digest('hex');
const receipt={at:new Date().toISOString(),hardware:{platform:process.platform,arch:process.arch},method:'Official pinned Aria MLX model; procedural original prompt; generated notes isolated by subtracting the tokenized prompt multiset',
  result,answerOffsetTicks:answerOffset,independentMIDI1Read,artifacts:{midiSha256:hash(midi),wavSha256:hash(wav)},
  limits:['Warm-filesystem single fixture, not a live-performance benchmark','Original prompt is procedural, not a human playing test','7-bit model velocity and tokenizer timing are not physical MIDI 2 precision','Studio continuation controls and hardware input/output qualification remain pending','Audio preview uses the procedural preview synth; listening quality is not accepted']};
for(const [name,value]of [['aria-project.json',JSON.stringify(source,null,2)+'\n'],['aria.json',JSON.stringify(receipt,null,2)+'\n'],['aria-continuation.mid',midi],['aria-continuation.wav',wav],['aria-continuation.ump.json',JSON.stringify(exportUMPTrace(source),null,2)+'\n']])await writeFile(new URL(name,directory),value);
process.stdout.write(`Aria generated ${result.notes.length} new notes from ${result.generatedTokens} tokens in ${result.generationSeconds}s; checkpoint and artifact checksums retained.\n`);
