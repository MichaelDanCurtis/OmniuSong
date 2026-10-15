import test from 'node:test';
import assert from 'node:assert/strict';
import { mkdtemp, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { editNote, restoreNote, deleteNote } from '../public/note-edit.js';
import { newProject, validateProject } from '../src/project.mjs';
import { Store } from '../src/storage.mjs';
import { exportArchive, importArchive } from '../src/archive.mjs';
import { exportUMPTrace } from '../src/midi.mjs';

function fixture(){
  const p=newProject('Precise human edits');p.bars=1;
  p.tracks[0].notes=[{id:'expressive',pitch:60,startTick:123,durationTicks:987,velocity16:49123,
    expression:[{type:'pitch',points:[{tick:0,value32:0x80000000},{tick:700,value32:0x87654321}]}],
    input:{sourceId:'synthetic:1',sourceName:'Synthetic test fixture',protocol:'midi2',group:3,channel:2,receivedTimeMs:10.25,keyDurationTicks:500,releaseVelocity16:0x9123,endReason:'pedal-release'},
    sourceEstimate:{pitch:60.01,velocity:.73},externalMetadata:{namespace:'preserved'}}];
  p.tracks[0].notes.push({...structuredClone(p.tracks[0].notes[0]),id:'repeated',startTick:321});
  p.providerMetadata={custom:'untouched'};
  p.inputCapture={version:1,droppedEvents:0,events:[{format:'ump',words:[0x43923c03,0xbfe3abcd],sourceId:'synthetic:1',sourceName:'Synthetic test fixture',hostTime:'18446744073709551615',receivedTimeMs:10.25,tick:123,trackId:p.tracks[0].id}]};
  return p;
}
const address=p=>[p.tracks[0].id,'expressive'];
test('precise edits preserve original performance, repeated notes, metadata and expression',()=>{
  const p=fixture(),before=structuredClone(p),next=editNote(p,...address(p),{pitch:65,startTick:3957,durationTicks:1201,velocity16:51312});
  assert.deepEqual(p,before);assert.equal(next.bars,2);assert.equal(validateProject(next),next);
  const n=next.tracks[0].notes[0],original=p.tracks[0].notes[0];
  for(const key of ['input','sourceEstimate','externalMetadata','expression'])assert.deepEqual(n[key],original[key]);
  assert.deepEqual(next.inputCapture,p.inputCapture);assert.deepEqual(next.tracks[1],p.tracks[1]);assert.deepEqual(next.tracks[0].notes[1],p.tracks[0].notes[1]);
  assert.deepEqual(n.editSource,{version:1,pitch:60,startTick:123,durationTicks:987,velocity16:49123,expression:original.expression});
  const again=editNote(next,...address(p),{velocity16:0});assert.deepEqual(again.tracks[0].notes[0].editSource,n.editSource);
  const restored=restoreNote(again,...address(p));assert.deepEqual(restored.tracks,p.tracks);assert.deepEqual(restored.inputCapture,p.inputCapture);
  assert.equal(restored.history.at(-1).action,'restore-note');assert.equal(restored.bars,2);
});
test('shortening cannot erase expression and the final point is allowed at note end',()=>{
  const p=fixture(),before=structuredClone(p);
  assert.throws(()=>editNote(p,...address(p),{durationTicks:699}),/at least 700 ticks/);assert.deepEqual(p,before);
  const next=editNote(p,...address(p),{durationTicks:700});assert.equal(validateProject(next),next);assert.deepEqual(next.tracks[0].notes[0].expression,p.tracks[0].notes[0].expression);
  const trace=exportUMPTrace(next);assert.ok(trace.packets.some(packet=>packet.tick===823&&packet.words[1]===0x87654321));
});
test('editor bounds retain MIDI 2 zero velocity and whole ticks without quantization',()=>{
  const p=fixture();
  for(const patch of [{pitch:128},{pitch:60.5},{startTick:-1},{startTick:NaN},{durationTicks:0},{velocity16:65536},{velocity16:''},{velocity16:1.5},{id:'replace'},{startTick:128*3840-700,durationTicks:701}])assert.throws(()=>editNote(p,...address(p),patch));
  const next=editNote(p,...address(p),{startTick:128*3840-700,durationTicks:700,velocity16:0});assert.equal(next.bars,128);assert.equal(next.tracks[0].notes[0].velocity16,0);assert.equal(validateProject(next),next);
  assert.equal(editNote(p,...address(p),{startTick:123,velocity16:49123}),p);
});
test('all note mutations enforce locks, source identity, and bounded history',()=>{
  const p=fixture();p.tracks[0].locked=true;
  for(const operation of [()=>editNote(p,...address(p),{pitch:61}),()=>restoreNote(p,...address(p)),()=>deleteNote(p,...address(p))])assert.throws(operation,/Unlock/);
  p.tracks[0].locked=false;assert.throws(()=>editNote(p,'missing','expressive',{}),/note track/);assert.throws(()=>deleteNote(p,p.tracks[0].id,'missing'),/Choose a note/);
  p.history=Array.from({length:5000},()=>({action:'test'}));assert.throws(()=>editNote(p,...address(p),{pitch:61}),/history limit/);
});
test('explicit deletion retains the entire removed note in the edit journal',()=>{
  const p=fixture(),next=deleteNote(p,...address(p));assert.equal(p.tracks[0].notes.length,2);assert.equal(next.tracks[0].notes.length,1);assert.equal(next.tracks[0].notes[0].id,'repeated');
  assert.deepEqual(next.history.at(-1).removedNote,p.tracks[0].notes[0]);assert.equal(validateProject(next),next);
});
test('invalid original snapshots are rejected by project validation',()=>{
  const p=fixture(),edited=editNote(p,...address(p),{pitch:61});
  for(const patch of [{version:2},{pitch:128},{durationTicks:1},{velocity16:-1},{editSource:{version:1}}]){
    const damaged=structuredClone(edited);Object.assign(damaged.tracks[0].notes[0].editSource,patch);assert.throws(()=>validateProject(damaged));
  }
});
test('saved edits and original snapshots survive a complete archive into another store',async t=>{
  const dir=await mkdtemp(join(tmpdir(),'omniusong-note-edit-'));t.after(()=>rm(dir,{recursive:true,force:true}));
  const source=await new Store(join(dir,'source')).init(),destination=await new Store(join(dir,'restored')).init(),p=fixture();
  const edited=editNote(p,...address(p),{pitch:63,startTick:4001,velocity16:51312}),saved=await source.saveProject(edited,null),bytes=await exportArchive(source,saved),restored=await importArchive(destination,bytes);
  assert.deepEqual(restored.project.tracks,saved.tracks);assert.deepEqual(restored.project.history,saved.history);assert.deepEqual(restored.project.inputCapture,p.inputCapture);
  assert.deepEqual(restoreNote(restored.project,...address(p)).tracks,p.tracks);
});
