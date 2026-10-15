import { randomUUID } from 'node:crypto';

export const PPQ = 960;
export const ASSET_ID = /^[a-f0-9]{64}\.(wav|webm|mp4|ogg|mp3|flac)$/;
export const uid = () => randomUUID();
export const copy = value => structuredClone(value);
export function demand(condition, message) { if (!condition) throw new Error(message); }
const finite = (n, min, max) => Number.isFinite(n) && n >= min && n <= max;
const int = (n, min, max) => Number.isInteger(n) && finite(n, min, max);
export function validateNote(note) {
  demand(typeof note.id === 'string' && note.id.length < 100, 'Note needs an id');
  demand(int(note.pitch, 0, 127), 'Pitch must be 0–127');
  demand(int(note.startTick, 0, 1e9), 'Invalid note start');
  demand(int(note.durationTicks, 1, 1e8), 'Invalid note duration');
  demand(int(note.velocity16, 0, 65535), 'Velocity must be 0–65535');
  if(note.input!==undefined){
    validateSource(note.input);
    demand(['local','midi1','midi2'].includes(note.input.protocol),'Invalid input protocol');
    demand(int(note.input.group,0,15)&&int(note.input.channel,0,15),'Invalid input address');
    demand(finite(note.input.receivedTimeMs,0,1e15),'Invalid input receipt time');
    if(note.input.keyDurationTicks!==undefined)demand(int(note.input.keyDurationTicks,1,1e8),'Invalid key duration');
    if(note.input.releaseVelocity16!==undefined)demand(int(note.input.releaseVelocity16,0,65535),'Invalid release velocity');
    if(note.input.endReason!==undefined)demand(typeof note.input.endReason==='string'&&note.input.endReason.length<=80,'Invalid note end reason');
  }
  for (const lane of note.expression ?? []) {
    demand(['pitch', 'pressure', 'timbre'].includes(lane.type), 'Unknown expression lane');
    demand(Array.isArray(lane.points) && lane.points.length <= 10000, 'Invalid expression points');
    let previous = -1;
    for (const p of lane.points) {
      demand(int(p.tick, 0, note.durationTicks) && p.tick >= previous, 'Expression points must be ordered inside note');
      demand(int(p.value32, 0, 0xffffffff), 'Expression must use unsigned 32 bit values');
      previous = p.tick;
    }
  }
}
function validateSource(source){
  demand(typeof source.sourceId==='string'&&source.sourceId.length>0&&source.sourceId.length<=200,'Invalid input source');
  demand(typeof source.sourceName==='string'&&source.sourceName.length<=160,'Invalid input source name');
}
export function validateInputCapture(capture,trackIds){
  demand(capture?.version===1&&Array.isArray(capture.events)&&capture.events.length<=4096,'Invalid raw input capture');
  demand(int(capture.droppedEvents,0,1e9),'Invalid dropped event count');
  for(const event of capture.events){
    validateSource(event);
    demand(trackIds.has(event.trackId)&&int(event.tick,0,1e9),'Invalid captured track or tick');
    demand(finite(event.receivedTimeMs,0,1e15),'Invalid capture receipt time');
    if(event.hostTime!==undefined)demand(typeof event.hostTime==='string'&&/^\d{1,20}$/.test(event.hostTime),'Invalid native host timestamp');
    if(event.format==='ump'){
      const counts=[1,1,1,2,2,4,1,1,2,2,2,3,3,4,4,4];
      demand(Array.isArray(event.words)&&event.words.length>=1&&event.words.length<=4&&event.words.every(w=>int(w,0,0xffffffff)),'Invalid captured UMP');
      demand(event.words.length===counts[event.words[0]>>>28],'Truncated captured UMP');
    }else demand(event.format==='midi1'&&Array.isArray(event.bytes)&&event.bytes.length>=1&&event.bytes.length<=1024&&event.bytes.every(b=>int(b,0,255)),'Invalid captured MIDI bytes');
  }
}
export function validateProject(project) {
  demand(project?.schemaVersion === 1, 'Unsupported project schema');
  demand(typeof project.id === 'string' && /^[a-zA-Z0-9-]{1,80}$/.test(project.id), 'Invalid project id');
  demand(typeof project.name === 'string' && project.name.trim().length > 0 && project.name.length <= 160, 'Invalid project name');
  demand(int(project.revision, 0, 1e9), 'Invalid project revision');
  demand(project.ppq === PPQ, 'Unsupported time resolution');
  demand(finite(project.tempo, 30, 300), 'Tempo must be 30–300 BPM');
  demand(int(project.bars, 1, 128), 'Bars must be 1–128');
  demand(Array.isArray(project.tracks) && project.tracks.length <= 64, 'Too many tracks');
  demand(typeof project.lyrics === 'string' && project.lyrics.length <= 50000, 'Invalid lyrics');
  const ids = new Set();
  for (const t of project.tracks) {
    demand(typeof t.id === 'string' && !ids.has(t.id), 'Duplicate or invalid track id'); ids.add(t.id);
    demand(typeof t.name === 'string' && t.name.length <= 160, 'Invalid track name');
    demand(['notes', 'audio'].includes(t.kind), 'Unknown track kind');
    demand(['keys', 'bass', 'drums', 'pad', 'sample', 'voice'].includes(t.instrument), 'Unknown instrument');
    demand(finite(t.gain, 0, 2), 'Invalid track gain');
    demand(typeof t.muted === 'boolean' && typeof t.locked === 'boolean', 'Invalid track switches');
    if(t.armed!==undefined)demand(typeof t.armed==='boolean','Invalid recording arm state');
    demand(Array.isArray(t.notes) && t.notes.length <= 20000, 'Invalid notes');
    const noteIds = new Set();
    for (const n of t.notes) { validateNote(n); demand(!noteIds.has(n.id), 'Duplicate note id'); noteIds.add(n.id); }
    if(t.audio!==undefined){
      demand(['microphone','import'].includes(t.audio.origin),'Invalid audio origin');
      demand(typeof t.audio.mimeType==='string'&&t.audio.mimeType.length<=100,'Invalid audio MIME');
      demand(finite(t.audio.durationSeconds,.001,86400),'Invalid audio duration');
      demand(int(t.audio.byteLength,1,30*1024*1024),'Invalid audio byte length');
      if(t.audio.originalName!==undefined)demand(typeof t.audio.originalName==='string'&&t.audio.originalName.length<=512,'Invalid original audio name');
    }
    if (t.assetId) demand(typeof t.assetId==='string'&&ASSET_ID.test(t.assetId), 'Invalid audio asset id');
    if (t.sourceAssetId) demand(typeof t.sourceAssetId==='string'&&ASSET_ID.test(t.sourceAssetId), 'Invalid source asset id');
  }
  demand(Array.isArray(project.history) && project.history.length <= 5000, 'Invalid project history');
  if(project.inputCapture!==undefined)validateInputCapture(project.inputCapture,ids);
  return project;
}
export function makeTrack(name, instrument = 'keys', notes = []) {
  return { id: uid(), name, kind: 'notes', instrument, gain: 0.65, muted: false, locked: false, armed:false, notes };
}
export function newProject(name = 'Midnight gardens') {
  return { schemaVersion: 1, id: uid(), revision: 0, ppq: PPQ, name, tempo: 104, bars: 4,
    key: 'C minor', lyrics: '', tracks: [makeTrack('Your piano', 'keys'), makeTrack('Pocket drums', 'drums')],
    history: [], createdAt: new Date().toISOString() };
}
export function mergeCandidate(project, result, provenance) {
  const next = copy(project);
  // Candidates append tracks and may extend the loop. Original performances and locks survive.
  if(result.minimumBars!==undefined){
    demand(int(result.minimumBars,1,128),'Invalid candidate session length');
    next.bars=Math.max(next.bars,result.minimumBars);
  }
  for (const track of result.tracks ?? []) next.tracks.push({ ...copy(track), id: uid(), provenance });
  if (result.lyrics !== undefined) {
    demand(!next.lyricsLocked, 'Lyrics are locked');
    next.lyrics = result.lyrics;
  }
  next.history.push({ id: uid(), at: new Date().toISOString(), action: 'accept-candidate', provenance });
  return validateProject(next);
}
