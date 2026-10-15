import { uid, makeTrack, PPQ } from './project.mjs';
export function random(seed=1) { let s=Number(seed)>>>0; return ()=>{s=(s*1664525+1013904223)>>>0;return s/4294967296;}; }
export function compose({bars=4,seed=42,density=0.55,root=48}={}) {
  const rand=random(seed), melody=[],bass=[],drums=[], chords=[];
  const scale=[0,2,3,5,7,8,10,12], progression=[0,8,3,10];
  const add=(notes,pitch,start,duration,vel=0.7)=>notes.push({id:uid(),pitch,startTick:start,durationTicks:duration,velocity16:Math.round(vel*65535),expression:[]});
  for(let b=0;b<bars;b++) {
    const offset=b*4*PPQ, degree=progression[b%4];
    for(const n of [0,3,7]) add(chords,root+degree+n,offset,PPQ*3.7|0,0.42);
    for(let beat=0;beat<4;beat++) {
      add(bass,root-12+degree,offset+beat*PPQ,PPQ*0.65|0,0.65);
      if(beat===0||beat===2) add(drums,36,offset+beat*PPQ,PPQ/8,0.85);
      if(beat===1||beat===3) add(drums,38,offset+beat*PPQ,PPQ/8,0.7);
      for(let half=0;half<2;half++) add(drums,42,offset+beat*PPQ+half*PPQ/2,PPQ/12,0.25+rand()*0.15);
    }
    for(let step=0;step<8;step++) if(rand()<density) add(melody,root+12+scale[Math.floor(rand()*scale.length)],offset+step*PPQ/2,PPQ/2,0.55+rand()*0.25);
  }
  return {tracks:[makeTrack('Sketch · melody','keys',melody),makeTrack('Sketch · harmony','pad',chords),makeTrack('Sketch · bass','bass',bass),makeTrack('Sketch · drums','drums',drums)],
    summary:'A seeded C minor arrangement from the local rule-based sketcher. No AI model was used.'};
}
