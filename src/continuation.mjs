import { demand, makeTrack, validateNote } from './project.mjs';

const BAR=3840;
export function ariaPhrase(project,input){
  const source=project.tracks.find(t=>t.id===input.sourceTrackId);
  demand(source?.kind==='notes'&&source.instrument!=='drums','Select a pitched note track for Aria');
  demand(Number.isInteger(input.startBar)&&input.startBar>=1&&input.startBar<=project.bars,'Choose a source start bar inside this session');
  demand(Number.isInteger(input.phraseBars)&&input.phraseBars>=1&&input.phraseBars<=4,'Choose 1–4 source bars');
  demand(input.startBar+input.phraseBars-1<=project.bars,'Source phrase extends past the session');
  demand(Number.isInteger(input.seed)&&input.seed>=0&&input.seed<=0xffffffff,'Seed must be a 32 bit unsigned integer');
  demand(Number.isInteger(input.maxNewTokens)&&input.maxNewTokens>=16&&input.maxNewTokens<=256,'Choose 16–256 response tokens');
  const startTick=(input.startBar-1)*BAR,endTick=startTick+input.phraseBars*BAR;
  demand((endTick-startTick)/960*60/project.tempo<=15,'Aria accepts up to 15 seconds of context. Choose fewer source bars or a faster tempo.');
  const overlapping=source.notes.filter(n=>n.startTick<endTick&&n.startTick+n.durationTicks>startTick);
  demand(overlapping.length>0,'The selected source bars contain no notes');
  demand(overlapping.length<=256,'Aria accepts up to 256 source notes. Choose a shorter phrase.');
  const notes=overlapping.map(n=>({pitch:n.pitch,startTick:Math.max(startTick,n.startTick)-startTick,
    durationTicks:Math.min(endTick,n.startTick+n.durationTicks)-Math.max(startTick,n.startTick),velocity16:n.velocity16}));
  const clippedNotes=overlapping.filter(n=>n.startTick<startTick||n.startTick+n.durationTicks>endTick).length;
  return {request:{tempo:project.tempo,seed:input.seed,maxNewTokens:input.maxNewTokens,notes},
    scope:{sourceTrackId:source.id,sourceTrackName:source.name,sourceProjectRevision:project.revision,startTick,endTick,sourceNoteCount:notes.length,clippedNotes,
      projection:'Pitched notes rendered as piano. Context is cropped at the selected bar boundaries, then tokenized with Aria timing and 7-bit velocity; expression is not used. Original source notes are unchanged.'}};
}
export function ariaCandidate(project,scope,result){
  demand(Array.isArray(result.notes)&&result.notes.length>0&&result.notes.length<=256,'Aria returned no complete bounded response');
  const notes=result.notes.map(note=>({...structuredClone(note),startTick:note.startTick+scope.endTick}));
  for(const note of notes)validateNote(note);
  const minimumBars=Math.ceil(Math.max(...notes.map(n=>n.startTick+n.durationTicks))/BAR);
  demand(minimumBars<=128,'Aria response exceeds the 128-bar session limit. Try fewer response tokens.');
  const {notes:unused,...evidence}=result;
  const track=makeTrack('AI answer · Aria MLX','keys',notes);
  track.continuation={...structuredClone(scope),answerStartTick:scope.endTick,...evidence};
  return {tracks:[track],model:result.model,modelRevision:result.modelRevision,modelSha256:result.modelSha256,minimumBars,auditionStartTick:scope.endTick,
    summary:`${notes.length} new piano notes from real local Aria MLX inference. Source: ${scope.sourceTrackName}, bars ${scope.startTick/BAR+1}–${scope.endTick/BAR}. Answer starts at bar ${scope.endTick/BAR+1}. Answer length is approximate.${minimumBars>project.bars?` Accepting extends the loop to ${minimumBars} bars.`:''}${scope.clippedNotes?` ${scope.clippedNotes} context note(s) cropped at phrase boundaries.`:''} Original performance preserved.`};
}
