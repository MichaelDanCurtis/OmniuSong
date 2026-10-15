import { randomUUID } from 'node:crypto';

export const PPQ = 960;
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
  demand(int(note.velocity16, 1, 65535), 'Velocity must be 1–65535');
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
    if (t.assetId) demand(/^[a-f0-9]{64}\.(wav|webm|mp4|ogg)$/.test(t.assetId), 'Invalid audio asset id');
    if (t.sourceAssetId) demand(/^[a-f0-9]{64}\.(wav|webm|mp4|ogg)$/.test(t.sourceAssetId), 'Invalid source asset id');
  }
  demand(Array.isArray(project.history) && project.history.length <= 5000, 'Invalid project history');
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
  // Candidates only append tracks. Existing human performances and locks survive.
  for (const track of result.tracks ?? []) next.tracks.push({ ...copy(track), id: uid(), provenance });
  if (result.lyrics !== undefined) {
    demand(!next.lyricsLocked, 'Lyrics are locked');
    next.lyrics = result.lyrics;
  }
  next.history.push({ id: uid(), at: new Date().toISOString(), action: 'accept-candidate', provenance });
  return validateProject(next);
}
