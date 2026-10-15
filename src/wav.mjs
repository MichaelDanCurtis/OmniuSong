import { demand } from './project.mjs';
export function encodeWAV(samples,sampleRate=44100) {
  const pcm=Buffer.alloc(samples.length*2);
  for(let i=0;i<samples.length;i++) pcm.writeInt16LE(Math.round(Math.max(-1,Math.min(1,samples[i]))*32767),i*2);
  const h=Buffer.alloc(44);h.write('RIFF');h.writeUInt32LE(36+pcm.length,4);h.write('WAVEfmt ',8);h.writeUInt32LE(16,16);h.writeUInt16LE(1,20);h.writeUInt16LE(1,22);h.writeUInt32LE(sampleRate,24);h.writeUInt32LE(sampleRate*2,28);h.writeUInt16LE(2,32);h.writeUInt16LE(16,34);h.write('data',36);h.writeUInt32LE(pcm.length,40);
  return Buffer.concat([h,pcm]);
}
export function renderSample({pitch=60,seconds=2,brightness=0.4,seed=1}={}) {
  demand(Number.isInteger(pitch)&&pitch>=0&&pitch<=127,'Invalid sample pitch');
  demand(Number.isFinite(seconds)&&seconds>=0.1&&seconds<=10,'Sample duration must be 0.1–10 seconds');
  demand(Number.isFinite(brightness)&&brightness>=0&&brightness<=1,'Invalid brightness');
  const sr=44100, f=440*2**((pitch-69)/12), samples=new Float32Array(Math.ceil(seconds*sr));
  for(let i=0;i<samples.length;i++) {
    const t=i/sr, env=Math.min(t/0.012,1)*Math.exp(-t*2.5)*Math.min((seconds-t)/0.05,1);
    samples[i]=0.4*env*Math.sin(2*Math.PI*f*t+brightness*3*Math.sin(2*Math.PI*f*2*t+seed%7));
  }
  return encodeWAV(samples,sr);
}
export function renderExamplePhrase(){
  const sr=44100,samples=new Float32Array(sr*3.2);
  for(const [n,pitch]of [60,64,67,72].entries()){
    const frequency=440*2**((pitch-69)/12);
    for(let i=0;i<sr*.65;i++){
      const time=i/sr,envelope=Math.min(time/.02,1)*Math.min((.65-time)/.05,1);
      samples[Math.round(n*.8*sr)+i]=.3*envelope*(Math.sin(2*Math.PI*frequency*time)+.3*Math.sin(4*Math.PI*frequency*time));
    }
  }
  return encodeWAV(samples,sr);
}
export function renderProject(project) {
  const sr=44100, duration=project.bars*4*60/project.tempo;
  demand(duration<=120,'Offline demo render is limited to 120 seconds');
  const samples=new Float32Array(Math.ceil((duration+1)*sr));
  for(const track of project.tracks.filter(t=>!t.muted&&t.kind==='notes')) for(const n of track.notes) {
    const at=n.startTick/960*60/project.tempo, length=n.durationTicks/960*60/project.tempo;
    if(at>=duration) continue;
    const f=440*2**((n.pitch-69)/12), count=Math.min(Math.ceil((length+0.12)*sr),samples.length-Math.floor(at*sr));
    for(let j=0;j<count;j++) {
      const t=j/sr, env=Math.min(t/0.01,1)*Math.min(Math.max(0,(length+0.12-t)/0.12),1);
      let value=Math.sin(2*Math.PI*f*t)*Math.exp(-t*(track.instrument==='pad'?0.5:2));
      if(track.instrument==='drums') value=n.pitch===36?Math.sin(2*Math.PI*(50*t+6*(1-Math.exp(-25*t))))*Math.exp(-t*20):Math.sin(j*173.31)*Math.sin(j*73.77)*Math.exp(-t*40);
      samples[Math.floor(at*sr)+j]+=value*env*n.velocity16/65535*track.gain*0.22;
    }
  }
  return encodeWAV(samples,sr);
}
