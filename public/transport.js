export function transportSettings(value){
  if(value===undefined)return {version:1,metronome:false,countInBars:0,clickLevel:.35};
  if(!value||value.version!==1||typeof value.metronome!=='boolean'||![0,1,2].includes(value.countInBars)||!Number.isFinite(value.clickLevel)||value.clickLevel<0||value.clickLevel>1)throw new Error('Invalid transport settings');
  return {...value};
}
export function transportClock(project,anchor,countInBars=0){
  if(!Number.isFinite(anchor)||!Number.isFinite(project.tempo)||project.tempo<30||project.tempo>300||project.ppq!==960||!Number.isInteger(project.bars)||project.bars<1||project.bars>128||![0,1,2].includes(countInBars))throw new Error('Invalid transport clock');
  const beatSeconds=60/project.tempo,secondsPerTick=beatSeconds/project.ppq,loopTicks=project.bars*3840,countInBeats=countInBars*4,startTime=anchor+countInBeats*beatSeconds;
  return {anchor,startTime,beatSeconds,secondsPerTick,loopTicks,loopSeconds:loopTicks*secondsPerTick,countInBeats,
    position(now){
      const started=now>=startTime,tick=started?((now-startTime)/secondsPerTick)%loopTicks:0;
      return {phase:started?'playing':countInBeats?'count-in':'starting',tick,bar:Math.floor(tick/3840)+1,beat:Math.floor(tick/960)%4+1,countInBeat:Math.max(1,Math.min(countInBeats,Math.floor((now-anchor)/beatSeconds)+1)),countInBeats};
    }};
}
