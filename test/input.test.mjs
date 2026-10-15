import test from 'node:test';
import assert from 'node:assert/strict';
import { PerformanceInput, decodeMIDI1, decodeUMP, scale7to16, appendCapture, CAPTURE_EVENT_LIMIT } from '../public/input.js';
import { newProject, validateProject } from '../src/project.mjs';
import { exportSMF, exportUMPTrace } from '../src/midi.mjs';

const address={sourceId:'device:a',sourceName:'Piano',protocol:'midi2',channel:0,group:0,pitch:60,velocity16:0x8123};
const turn=()=>new Promise(resolve=>setImmediate(resolve));
function fixture(options={}){
  let time=100,id=0;const starts=[],ends=[],releases=[],keys=[];
  const input=new PerformanceInput({now:()=>time,id:()=>String(++id),startVoice:v=>{starts.push(v);return ()=>releases.push(v.id);},onEnd:(v,reason)=>ends.push({v,reason}),onKeyRelease:v=>keys.push(v.id),...options});
  return {input,starts,ends,releases,keys,setTime:value=>time=value};
}
test('same pitches remain independent across source, channel and UMP group',async()=>{
  const f=fixture();for(const event of [address,{...address,sourceId:'device:b'},{...address,channel:1},{...address,group:1}])f.input.noteOn(event);
  await turn();f.input.noteOff(address);assert.equal(f.input.voices.size,3);assert.deepEqual(f.releases,['1']);
  f.input.releaseSource('device:a');assert.equal(f.input.voices.size,1);assert.equal([...f.input.voices.values()][0].sourceId,'device:b');
});
test('repeated same-pitch notes get distinct identities and FIFO note-off pairing',async()=>{
  const f=fixture();assert.equal(f.input.noteOn(address),'1');assert.equal(f.input.noteOn(address),'2');await turn();
  f.input.noteOff(address);assert.deepEqual(f.releases,['1']);f.input.noteOff(address);assert.deepEqual(f.releases,['1','2']);
});
test('pointer note identities allow reverse release without ending another held key',async()=>{
  const f=fixture(),first=f.input.noteOn(address),second=f.input.noteOn(address);await turn();
  f.input.noteOff({...address,voiceId:second});f.input.noteOff({...address,voiceId:second});
  assert.deepEqual(f.releases,[second]);assert.deepEqual([...f.input.voices.keys()],[first]);
});
test('bounded polyphony reports saturation and panic makes input usable again',async()=>{
  const errors=[],f=fixture({maxVoices:2,onError:e=>errors.push(e.message)});f.input.noteOn(address);f.input.noteOn(address);f.input.noteOn(address);
  assert.equal(f.input.voices.size,2);assert.equal(errors.length,1);f.input.panic();assert.ok(f.input.noteOn(address));f.input.panic();await turn();
});
test('sustain is scoped to device, group and channel and retains physical release timing',async()=>{
  const f=fixture();f.input.sustain(address,true);f.input.noteOn(address);f.input.noteOn({...address,channel:1});await turn();
  f.setTime(250);f.input.noteOff(address);f.input.noteOff({...address,channel:1});assert.equal(f.input.voices.size,1);assert.deepEqual(f.keys,['1','2']);
  assert.equal([...f.input.voices.values()][0].keyReleasedAt,250);f.setTime(600);f.input.sustain(address,false);
  assert.equal(f.ends.at(-1).v.endedAt,600);assert.equal(f.ends.at(-1).reason,'pedal-release');
});
test('a fast release records the note but cannot start a late voice',async()=>{
  let resolveAudio;const recorded=[],f=fixture({onStart:v=>recorded.push(v.id),startVoice:()=>new Promise(r=>resolveAudio=r)});
  f.input.noteOn(address);await turn();f.input.noteOff(address);let released=0;resolveAudio(()=>released++);await turn();
  assert.deepEqual(recorded,['1']);assert.equal(released,1);assert.equal(f.input.voices.size,0);
  const immediate=fixture();immediate.input.noteOn(address);immediate.input.noteOff(address);await turn();assert.equal(immediate.starts.length,0);
});
test('panic cancels pending startup, clears pedals, and ends each recording once',async()=>{
  const f=fixture();f.input.sustain(address,true);f.input.noteOn(address);f.input.noteOff(address);f.input.panic('focus-lost');f.input.panic();await turn();
  assert.equal(f.input.voices.size,0);assert.equal(f.input.pedals.size,0);assert.equal(f.ends.length,1);assert.equal(f.ends[0].reason,'focus-lost');assert.equal(f.starts.length,0);
});
test('disconnect releases sustained notes and pedal state only for that endpoint',async()=>{
  const f=fixture();f.input.sustain(address,true);f.input.sustain({...address,sourceId:'device:b'},true);f.input.noteOn(address);await turn();f.input.noteOff(address);f.input.releaseSource(address.sourceId);
  assert.equal(f.input.voices.size,0);assert.equal(f.input.pedals.size,1);assert.equal(f.ends[0].reason,'device-disconnected');
});
test('audio load failure closes the voice and completes its recorded lifetime',async()=>{
  const errors=[],f=fixture({startVoice:async()=>{throw new Error('missing audio');},onError:e=>errors.push(e.message)});f.input.noteOn(address);await turn();
  assert.equal(f.input.voices.size,0);assert.equal(f.ends[0].reason,'audio-error');assert.deepEqual(errors,['missing audio']);
});
test('all notes off respects sustain; all sound off releases the channel immediately',async()=>{
  const f=fixture();f.input.noteOn(address);f.input.sustain(address,true);await turn();f.input.control({...address,controller:123});assert.equal(f.input.voices.size,1);
  f.input.control({...address,controller:120});assert.equal(f.input.voices.size,0);assert.equal(f.ends[0].reason,'all-sound-off');
});
test('MIDI 1 uses center-preserving scaling and zero Note On means release',()=>{
  assert.deepEqual([0,32,64,70,96,100,120,127].map(scale7to16),[0,16384,32768,35888,49412,51492,61895,65535]);
  assert.equal(decodeMIDI1([0x92,60,0]).kind,'off');assert.equal(decodeMIDI1([0x82,60,64]).releaseVelocity16,32768);
  assert.equal(decodeMIDI1([0xb1,64,64]).down,true);assert.equal(decodeMIDI1([0xb1,64,63]).down,false);
  assert.throws(()=>decodeMIDI1([0x90,60]),/Malformed/);assert.throws(()=>decodeMIDI1([0x90,255,30]),/Malformed/);
});
test('UMP decoding retains every word, unknown packets, native velocity zero and controller resolution',()=>{
  const words=[0x23923c64,0x45913c03,0x0000abcd,0x45b14000,0x80000000,0xd0000000,1,2,3];
  const packets=decodeUMP(words);assert.deepEqual(packets.flatMap(p=>p.words),words);assert.equal(packets.length,4);
  assert.equal(packets[0].event.group,3);assert.equal(packets[0].event.channel,2);assert.equal(packets[0].event.velocity16,51492);
  assert.equal(packets[1].event.kind,'on');assert.equal(packets[1].event.velocity16,0);assert.equal(packets[2].event.down,true);assert.equal(packets[3].event,null);
  assert.throws(()=>decodeUMP([0x40903c00]),/Truncated/);assert.throws(()=>decodeUMP([-1]),/Invalid/);
});
test('raw capture is bounded, preserves host timestamp precision and validates project round trips',()=>{
  const p=newProject(),event={format:'ump',sourceId:address.sourceId,sourceName:'Piano',words:[0x45913c03,0x8123abcd],receivedTimeMs:1234.5,hostTime:'18446744073709551615',tick:17,trackId:p.tracks[0].id};
  assert.equal(appendCapture(p,event),true);event.words[1]=0;assert.equal(p.inputCapture.events[0].words[1],0x8123abcd);
  assert.equal(validateProject(structuredClone(p)).inputCapture.events[0].hostTime,'18446744073709551615');
  assert.deepEqual(exportUMPTrace(p).rawInputCapture,p.inputCapture);
  while(p.inputCapture.events.length<CAPTURE_EVENT_LIMIT)appendCapture(p,event);
  assert.equal(appendCapture(p,event),false);assert.equal(p.inputCapture.droppedEvents,1);
  p.inputCapture.events[0].words.pop();assert.throws(()=>validateProject(p),/Truncated/);
});
test('captured zero velocity and release velocity survive project and UMP export; MIDI 1 reports coercion',()=>{
  const p=newProject();p.tracks[0].notes.push({id:'observed',pitch:60,startTick:17,durationTicks:200,velocity16:0,expression:[],input:{...address,receivedTimeMs:50,keyDurationTicks:100,releaseVelocity16:0x9876,endReason:'pedal-release'}});
  validateProject(p);const trace=exportUMPTrace(p);assert.equal(trace.packets[0].words[1],0);assert.equal(trace.packets[1].words[1],0x98760000);
  assert.ok(exportSMF(p).warnings.some(w=>w.includes('zero-velocity')));
});
