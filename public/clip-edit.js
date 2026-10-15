const keys=['startTick','trimStartSeconds','trimEndSeconds'];
const demand=(condition,message)=>{if(!condition)throw new Error(message);};
export function clipWindow(audio){
  demand(Number.isFinite(audio?.durationSeconds)&&audio.durationSeconds>=.001&&audio.durationSeconds<=86400,'Invalid audio duration');
  const startTick=audio.startTick===undefined?0:audio.startTick,start=audio.trimStartSeconds===undefined?0:audio.trimStartSeconds,end=audio.trimEndSeconds===undefined?audio.durationSeconds:audio.trimEndSeconds;
  demand(Number.isInteger(startTick)&&startTick>=0&&startTick<=1e9,'Invalid audio start');
  demand(Number.isFinite(start)&&Number.isFinite(end)&&start>=0&&end<=audio.durationSeconds&&end>start&&end-start+1e-9>=.001,'Choose a source window of at least 0.001 seconds within the original audio');
  return {startTick,trimStartSeconds:start,trimEndSeconds:end,durationSeconds:end-start};
}
export function originalClipAudio(audio){
  demand(audio.editSource?.version===1&&audio.editSource.original&&typeof audio.editSource.original==='object'&&!Array.isArray(audio.editSource.original),'Invalid original clip snapshot');
  demand(Object.keys(audio.editSource.original).every(key=>keys.includes(key)),'Invalid original clip settings');
  const original=structuredClone(audio);for(const key of [...keys,'editSource'])delete original[key];
  Object.assign(original,audio.editSource.original);clipWindow(original);return original;
}
function selected(project,trackId){
  const track=project.tracks.find(t=>t.id===trackId);
  demand(track?.kind==='audio'&&track.audio&&track.assetId,'Choose an audio clip');
  demand(!track.locked,'Unlock this track before editing its clip');
  demand(project.history.length<5000,'This session has reached its edit history limit');return track;
}
function saveEdit(project,trackId,audio,action){
  const view=clipWindow(audio),endTick=view.startTick+view.durationSeconds*project.tempo/60*project.ppq;
  demand(endTick<=128*3840,'The clip must end within 128 bars');
  const next=structuredClone(project),track=next.tracks.find(t=>t.id===trackId),before=clipWindow(track.audio);
  track.audio=audio;next.bars=Math.max(next.bars,Math.ceil(endTick/3840));
  next.history.push({id:crypto.randomUUID(),at:new Date().toISOString(),action,trackId,before,after:view});return next;
}
export function editClip(project,trackId,patch){
  const track=selected(project,trackId);
  demand(patch&&Object.keys(patch).every(key=>keys.includes(key)),'Only placement and source trim can be changed here');
  demand(Object.values(patch).every(Number.isFinite),'Clip values must be finite numbers');
  const audio={...structuredClone(track.audio),...patch},before=clipWindow(track.audio),after=clipWindow(audio);
  if(keys.every(key=>before[key]===after[key]))return project;
  audio.editSource??={version:1,original:Object.fromEntries(keys.filter(key=>Object.hasOwn(track.audio,key)).map(key=>[key,track.audio[key]]))};
  return saveEdit(project,trackId,audio,'edit-clip');
}
export function restoreClip(project,trackId){
  const track=selected(project,trackId);if(!track.audio.editSource)return project;
  return saveEdit(project,trackId,originalClipAudio(track.audio),'restore-clip');
}
export function analysisWindow(buffer,audio){
  const {sampleRate,length}=buffer;
  demand(Number.isInteger(sampleRate)&&sampleRate>=8000&&sampleRate<=384000&&Number.isInteger(length)&&length>0&&length<=sampleRate*120,'Choose decoded audio no longer than two minutes');
  const view=clipWindow(audio),startFrame=Math.min(length,Math.round(view.trimStartSeconds*sampleRate)),endFrame=Math.min(length,Math.round(view.trimEndSeconds*sampleRate));
  demand(endFrame>startFrame,'The source window is outside the decoded audio');
  return {sampleRate,decodedLength:length,startFrame,endFrame,requestedStartSeconds:view.trimStartSeconds,requestedEndSeconds:view.trimEndSeconds,startSeconds:startFrame/sampleRate,endSeconds:endFrame/sampleRate};
}
export function validateAnalysisWindow(audio,window){
  demand(window&&typeof window==='object','Prepare this trimmed clip for transcription first');
  const expected=analysisWindow({sampleRate:window.sampleRate,length:window.decodedLength},audio);
  demand(Object.entries(expected).every(([key,value])=>window[key]===value),'Source trim changed; prepare the audio again');return expected;
}
