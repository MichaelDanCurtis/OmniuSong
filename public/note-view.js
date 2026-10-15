import { clipWindow } from './clip-edit.js';
const BAR=3840;
export function rollWindow(bars,page=0,topPitch=72){
  const pages=Math.ceil(bars/4),index=Math.max(0,Math.min(pages-1,page));
  return {page:index,pages,startTick:index*4*BAR,startBar:index*4+1,endBar:Math.min(bars,(index+1)*4),topPitch:Math.max(24,Math.min(127,topPitch))};
}
export function candidateAudition(project,result){
  const preview=structuredClone(project),start=result.auditionStartTick??0;
  preview.tracks=structuredClone(result.tracks??[]);
  for(const t of preview.tracks){
    for(const n of t.notes)n.startTick-=start;
    if(t.kind==='audio')t.audio={...t.audio,startTick:(t.audio?.startTick??0)-start};
  }
  const end=Math.max(1,...preview.tracks.flatMap(t=>t.kind==='audio'?[(t.audio?.startTick??0)+(t.audio?clipWindow(t.audio).durationSeconds:0)*project.tempo/60*960]:t.notes.map(n=>n.startTick+n.durationTicks)));
  preview.bars=result.auditionStartTick===undefined?Math.max(project.bars,Math.ceil(end/BAR)):Math.ceil(end/BAR);
  return preview;
}
