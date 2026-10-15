import { PPQ, validateProject, demand } from './project.mjs';

export function variableLength(value) {
  demand(Number.isInteger(value) && value >= 0 && value <= 0x0fffffff, 'MIDI delta out of range');
  const out = [value & 127];
  while ((value = Math.floor(value / 128))) out.unshift((value & 127) | 128);
  return out;
}
const u16 = v => [(v >>> 8) & 255, v & 255];
const u32 = v => [(v >>> 24) & 255, (v >>> 16) & 255, (v >>> 8) & 255, v & 255];
const chunk = (name, bytes) => Buffer.concat([Buffer.from(name), Buffer.from(u32(bytes.length)), Buffer.from(bytes)]);

export function exportSMF(project) {
  validateProject(project);
  const warnings = [];
  const tempo = Math.round(60e6 / project.tempo);
  const conductor = chunk('MTrk', [0,255,81,3,...u32(tempo).slice(1),0,255,88,4,4,2,24,8,0,255,47,0]);
  let melodicIndex = 0;
  const tracks = project.tracks.filter(t => t.kind === 'notes').map(track => {
    const channel = track.instrument === 'drums' ? 9 : (melodicIndex++ % 15 >= 9 ? ((melodicIndex - 1) % 15) + 1 : (melodicIndex - 1) % 15);
    const name = [...Buffer.from(track.name, 'utf8').subarray(0,120)];
    const program = { keys: 0, bass: 33, pad: 89, sample: 0, voice: 53 }[track.instrument] ?? 0;
    const data = [0,255,3,...variableLength(name.length),...name,0,0xc0 | channel,program];
    const events = [];
    for (const n of track.notes) {
      events.push({ tick: n.startTick, order: 1, bytes: [0x90 | channel,n.pitch,Math.max(1,Math.round(n.velocity16 / 65535 * 127))] });
      events.push({ tick: n.startTick + n.durationTicks, order: 0, bytes: [0x80 | channel,n.pitch,0] });
      if (n.expression?.length) warnings.push(`${track.name}: per-note expression retained in project; omitted from MIDI 1 export`);
    }
    events.sort((a,b) => a.tick-b.tick || a.order-b.order || a.bytes[1]-b.bytes[1]);
    let last = 0;
    for (const e of events) { data.push(...variableLength(e.tick-last),...e.bytes); last=e.tick; }
    data.push(0,255,47,0);
    return chunk('MTrk', data);
  });
  if (project.tracks.some(t => t.kind === 'audio')) warnings.push('Audio tracks do not become notes in a MIDI export');
  if (melodicIndex > 15) warnings.push('More than 15 melodic tracks reuse MIDI channels; split the export to preserve instrument programs');
  return { bytes: Buffer.concat([chunk('MThd', [...u16(1),...u16(tracks.length+1),...u16(PPQ)]),conductor,...tracks]), warnings: [...new Set(warnings)] };
}
export function noteUMP({ pitch, velocity16, channel=0, group=0, on=true }) {
  demand(Number.isInteger(pitch) && pitch>=0 && pitch<128, 'Invalid MIDI note');
  demand(Number.isInteger(velocity16) && velocity16>=0 && velocity16<=65535, 'Invalid MIDI 2 velocity');
  demand(Number.isInteger(channel) && channel>=0 && channel<16 && Number.isInteger(group) && group>=0 && group<16, 'Invalid MIDI address');
  return [((4<<28)|(group<<24)|((on?9:8)<<20)|(channel<<16)|(pitch<<8))>>>0,(velocity16<<16)>>>0];
}
export function perNotePitchUMP(pitch, value32, channel=0, group=0) {
  noteUMP({pitch,velocity16:0,channel,group});
  demand(Number.isInteger(value32) && value32>=0 && value32<=0xffffffff,'Invalid per-note pitch value');
  return [((4<<28)|(group<<24)|(6<<20)|(channel<<16)|(pitch<<8))>>>0,value32>>>0];
}
export function exportUMPTrace(project) {
  validateProject(project);
  const packets=[];
  let i=0;
  for (const t of project.tracks.filter(t=>t.kind==='notes')) {
    const address={channel:t.instrument==='drums'?9:(i%15>=9?i%15+1:i%15),group:Math.floor(i/15)}; i++;
    for (const n of t.notes) {
      packets.push({ tick:n.startTick, order:1, trackId:t.id, words:noteUMP({...n,...address}) });
      for (const lane of n.expression??[]) if(lane.type==='pitch') for (const p of lane.points)
        packets.push({tick:n.startTick+p.tick,order:2,trackId:t.id,words:perNotePitchUMP(n.pitch,p.value32,address.channel,address.group)});
      packets.push({tick:n.startTick+n.durationTicks,order:0,trackId:t.id,words:noteUMP({...n,...address,on:false})});
    }
  }
  return { format:'omniusong-ump-trace', version:1, protocol:'MIDI 2.0', ppq:PPQ, tempo:project.tempo,
    description:'Timestamped UMP JSON interchange. This is not a Standard MIDI Clip File.',
    warnings:project.tracks.flatMap(t=>t.notes).some(n=>n.expression?.some(e=>e.type!=='pitch'))?['Pressure and timbre lanes remain in the project; their UMP encoding is pending']:[],
    packets:packets.sort((a,b)=>a.tick-b.tick||a.order-b.order).map(({order,...p})=>p) };
}
