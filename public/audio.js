export class StudioAudio {
  constructor(){this.voices=new Map();this.buffers=new Map();this.playing=false;this.timer=null;this.generation=0;}
  async ready(){
    if(!this.context){this.context=new AudioContext({latencyHint:'interactive'});this.master=this.context.createGain();this.master.gain.value=0.55;const limiter=this.context.createDynamicsCompressor();limiter.threshold.value=-10;limiter.ratio.value=12;this.master.connect(limiter);limiter.connect(this.context.destination);}
    await this.context.resume();return this.context;
  }
  async loadAsset(id){
    if(!this.buffers.has(id)){const response=await fetch('/api/assets/'+id);if(!response.ok)throw new Error('Could not load audio');const bytes=await response.arrayBuffer();this.buffers.set(id,await this.context.decodeAudioData(bytes));}
    return this.buffers.get(id);
  }
  voice(pitch,velocity=0.7,instrument='keys',at=this.context.currentTime,duration=null,buffer=null,root=60,trackGain=0.65,trackId=null){
    const ctx=this.context,id=crypto.randomUUID(),gain=ctx.createGain();gain.connect(this.master);
    const max=0.3*velocity*trackGain;gain.gain.setValueAtTime(0,at);gain.gain.linearRampToValueAtTime(max,at+0.008);
    let source;
    if(buffer){source=ctx.createBufferSource();source.buffer=buffer;source.playbackRate.value=2**((pitch-root)/12);}
    else if(instrument==='drums'&&pitch!==36){
      source=ctx.createBufferSource();const noise=ctx.createBuffer(1,ctx.sampleRate*0.25,ctx.sampleRate),out=noise.getChannelData(0);for(let i=0;i<out.length;i++)out[i]=Math.random()*2-1;source.buffer=noise;
      const filter=ctx.createBiquadFilter();filter.type='highpass';filter.frequency.value=pitch===38?1200:6500;source.connect(filter);filter.connect(gain);gain.gain.exponentialRampToValueAtTime(0.0001,at+(pitch===38?0.18:0.07));
    }else{
      source=ctx.createOscillator();source.type=instrument==='bass'?'triangle':instrument==='pad'?'sine':'triangle';const frequency=440*2**((pitch-69)/12);source.frequency.setValueAtTime(instrument==='drums'?145:frequency,at);
      if(instrument==='drums'){source.frequency.exponentialRampToValueAtTime(42,at+0.12);gain.gain.exponentialRampToValueAtTime(0.0001,at+0.25);}
      else if(instrument==='keys')gain.gain.exponentialRampToValueAtTime(Math.max(0.0001,max*0.25),at+0.6);
    }
    if(!(instrument==='drums'&&pitch!==36&&!buffer))source.connect(gain);
    source.start(at);
    const release=(time=ctx.currentTime)=>{if(!this.voices.has(id))return;const end=Math.max(time,at+0.015);gain.gain.cancelAndHoldAtTime(end);gain.gain.linearRampToValueAtTime(0,end+0.1);try{source.stop(end+0.11);}catch{}};
    this.voices.set(id,{source,gain,trackId});source.onended=()=>{this.voices.delete(id);source.disconnect();gain.disconnect();};
    if(duration!==null)release(at+duration);else if(instrument==='drums')release(at+0.25);
    return release;
  }
  async play(project,onPosition=()=>{}){
    this.stop();const generation=++this.generation;await this.ready();
    const assets=[...new Set(project.tracks.filter(t=>t.assetId&&!t.muted).map(t=>t.assetId))];
    await Promise.all(assets.map(id=>this.loadAsset(id)));
    if(generation!==this.generation)return;
    this.playing=true;this.project=structuredClone(project);this.mutes=new Map(project.tracks.map(t=>[t.id,t.muted]));this.startTime=this.context.currentTime+0.06;
    const secondsPerTick=60/project.tempo/project.ppq,loopTicks=project.bars*4*project.ppq,loopSeconds=loopTicks*secondsPerTick;
    const events=[];
    for(const t of project.tracks){
      if(t.kind==='audio'&&t.assetId)events.push({tick:0,track:t,audio:true});
      else for(const n of t.notes)if(n.startTick<loopTicks)events.push({tick:n.startTick,track:t,note:n});
    }
    events.sort((a,b)=>a.tick-b.tick);let loop=0,index=0;
    const schedule=()=>{
      if(!this.playing)return;
      const now=this.context.currentTime;
      while(events.length){
        const e=events[index],at=this.startTime+loop*loopSeconds+e.tick*secondsPerTick;
        if(at>now+0.13)break;
        if(at>=now-0.05&&!this.mutes.get(e.track.id)){
          if(e.audio){const src=this.context.createBufferSource();src.buffer=this.buffers.get(e.track.assetId);const g=this.context.createGain();g.gain.value=e.track.gain;src.connect(g);g.connect(this.master);src.start(Math.max(now,at));src.stop(Math.max(now,at)+Math.min(src.buffer.duration,loopSeconds));const id=crypto.randomUUID();this.voices.set(id,{source:src,gain:g,trackId:e.track.id});src.onended=()=>{this.voices.delete(id);src.disconnect();g.disconnect();};}
          else this.voice(e.note.pitch,e.note.velocity16/65535,e.track.instrument,Math.max(now,at),Math.min(e.note.durationTicks*secondsPerTick,loopSeconds),this.buffers.get(e.track.assetId),e.track.sampleRoot??60,e.track.gain,e.track.id);
        }
        index++;if(index===events.length){index=0;loop++;}
      }
      onPosition(Math.max(0,(now-this.startTime)/secondsPerTick)%loopTicks);
      this.timer=setTimeout(schedule,25);
    };schedule();
  }
  tick(){if(!this.playing)return 0;return Math.max(0,(this.context.currentTime-this.startTime)*this.project.tempo/60*this.project.ppq)%(this.project.bars*4*this.project.ppq);}
  setTrackMuted(id,muted){if(this.mutes)this.mutes.set(id,muted);if(muted&&this.context)for(const voice of this.voices.values())if(voice.trackId===id){voice.gain.gain.cancelScheduledValues(this.context.currentTime);voice.gain.gain.setTargetAtTime(0,this.context.currentTime,0.005);}}
  stop(){this.generation++;this.playing=false;clearTimeout(this.timer);for(const {source,gain}of this.voices.values()){try{gain.gain.cancelScheduledValues(0);gain.gain.value=0;source.stop();}catch{}}this.voices.clear();}
}
export function bufferWAV(buffer){
  const length=buffer.length,bytes=new ArrayBuffer(44+length*2),view=new DataView(bytes),text=(at,s)=>{for(let i=0;i<s.length;i++)view.setUint8(at+i,s.charCodeAt(i));};
  text(0,'RIFF');view.setUint32(4,36+length*2,true);text(8,'WAVEfmt ');view.setUint32(16,16,true);view.setUint16(20,1,true);view.setUint16(22,1,true);view.setUint32(24,buffer.sampleRate,true);view.setUint32(28,buffer.sampleRate*2,true);view.setUint16(32,2,true);view.setUint16(34,16,true);text(36,'data');view.setUint32(40,length*2,true);
  const channels=Array.from({length:buffer.numberOfChannels},(_,i)=>buffer.getChannelData(i));
  for(let i=0;i<length;i++){let sample=0;for(const channel of channels)sample+=channel[i]/channels.length;view.setInt16(44+i*2,Math.round(Math.max(-1,Math.min(1,sample))*32767),true);}
  return bytes;
}
