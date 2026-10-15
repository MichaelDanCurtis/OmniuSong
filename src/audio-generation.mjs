import { demand, makeTrack } from './project.mjs';

export function magentaScope(project,input){
  demand(typeof input.prompt==='string'&&input.prompt.trim().length>0&&input.prompt.trim().length<=800,'Describe the sound in 1–800 characters');
  demand(Number.isInteger(input.seed)&&input.seed>=0&&input.seed<=0xffffffff,'Seed must be a 32 bit unsigned integer');
  demand([2,4,8].includes(input.durationSeconds),'Choose 2, 4, or 8 seconds of audio');
  demand(Number.isInteger(input.startBar)&&input.startBar>=1&&input.startBar<=project.bars,'Choose a start bar inside this session');
  demand(typeof input.guideWithNotes==='boolean','Choose whether to use a note guide');
  const startTick=(input.startBar-1)*3840,ticksPerSecond=project.tempo/60*960;
  const endTick=startTick+Math.ceil(input.durationSeconds*ticksPerSecond),minimumBars=Math.ceil(endTick/3840);
  demand(minimumBars<=128,'Audio would extend past 128 bars. Choose an earlier start or shorter duration.');
  const scope={startTick,endTick,minimumBars,tempo:project.tempo,durationSeconds:input.durationSeconds,sourceProjectRevision:project.revision,guideWithNotes:input.guideWithNotes,sourceNoteCount:0,clippedNotes:0,
    projection:'Optional pitched note guidance at 25 Hz; velocity and expression are not passed. Notes and tempo are guidance, not exact constraints. Source performance is unchanged.'};
  const request={prompt:input.prompt.trim(),seed:input.seed,durationSeconds:input.durationSeconds,tempo:project.tempo,notes:[]};
  if(input.guideWithNotes){
    const source=project.tracks.find(t=>t.id===input.sourceTrackId);
    demand(source?.kind==='notes'&&source.instrument!=='drums','Select a pitched note track to guide Magenta');
    const sourceEndTick=Math.min(endTick,project.bars*3840);
    const notes=source.notes.filter(n=>n.startTick<sourceEndTick&&n.startTick+n.durationTicks>startTick);
    demand(notes.length>0,'No source notes overlap this audio window. Choose another start bar or turn off the note guide.');
    demand(notes.length<=256,'Magenta accepts up to 256 guide notes. Choose a shorter audio window.');
    request.notes=notes.map(n=>({pitch:n.pitch,startTick:Math.max(startTick,n.startTick)-startTick,
      durationTicks:Math.min(sourceEndTick,n.startTick+n.durationTicks)-Math.max(startTick,n.startTick)}));
    // The worker uses exact seconds; rounding a fractional final tick upward must
    // not produce a context note beyond that duration.
    const lastTick=Math.floor(input.durationSeconds*ticksPerSecond);
    request.notes=request.notes.filter(n=>n.startTick<lastTick).map(n=>({...n,durationTicks:Math.min(n.durationTicks,lastTick-n.startTick)}));
    demand(request.notes.length>0,'No guide notes fit this audio window');
    Object.assign(scope,{sourceTrackId:source.id,sourceTrackName:source.name,sourceEndTick,sourceNoteCount:request.notes.length,guideNotes:structuredClone(request.notes),
      clippedNotes:notes.filter(n=>n.startTick<startTick||n.startTick+n.durationTicks>Math.min(sourceEndTick,startTick+lastTick)).length});
  }
  return {request,scope};
}
export function magentaCandidate(project,scope,result,assetId){
  const track={...makeTrack('AI audio · Magenta Small','sample'),kind:'audio',assetId,
    audio:{origin:'generated',mimeType:'audio/wav',durationSeconds:result.durationSeconds,byteLength:result.audioBytes,originalName:`Magenta-${result.seed}.wav`,startTick:scope.startTick},
    generation:{scope:structuredClone(scope),...structuredClone(result)}};
  return {tracks:[track],model:result.model,modelRevision:result.modelRevision,modelSha256:result.modelSha256,
    minimumBars:scope.minimumBars,auditionStartTick:scope.startTick,
    summary:`${result.durationSeconds} seconds of real local Magenta audio, placed at bar ${scope.startTick/3840+1}.${scope.guideWithNotes?` Guided by ${scope.sourceNoteCount} notes from ${scope.sourceTrackName}.`:' Style prompt only.'}${scope.clippedNotes?` ${scope.clippedNotes} guide note(s) cropped at the window edges.`:''}${scope.minimumBars>project.bars?` Accepting extends the loop to ${scope.minimumBars} bars.`:''} Original performances stay intact. Tempo and notes are guidance.`};
}
