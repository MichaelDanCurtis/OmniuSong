import { clipWindow, clipGainPoints } from './clip-edit.js';
import { transportSettings, transportClock } from './transport.js';
export class StudioAudio {
  constructor(){this.voices=new Map();this.buffers=new Map();this.playing=false;this.timer=null;this.generation=0;this.phase='stopped';this.clock=null;this.onPosition=()=>{};}
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
  click(at,accent,kind){
    if(this.clickLevel===0)return;
    const ctx=this.context,source=ctx.createOscillator(),gain=ctx.createGain(),id=crypto.randomUUID();
    source.type='sine';source.frequency.setValueAtTime(accent?1760:1100,at);gain.gain.setValueAtTime(0,at);gain.gain.linearRampToValueAtTime(.18*this.clickLevel,at+.001);gain.gain.exponentialRampToValueAtTime(.0001,at+.035);
    source.connect(gain);gain.connect(this.master);source.start(at);source.stop(at+.045);this.voices.set(id,{source,gain,trackId:null,clickKind:kind});source.onended=()=>{this.voices.delete(id);source.disconnect();gain.disconnect();};
  }
  setMetronome(enabled,level=this.clickLevel){
    this.clickEnabled=this.mode==='audition'?false:enabled;this.clickLevel=level;
    for(const [id,voice]of this.voices)if(voice.clickKind&&(level===0||!this.clickEnabled&&voice.clickKind==='song')){try{voice.gain.gain.cancelScheduledValues(0);voice.gain.gain.value=0;voice.source.stop();}catch{}this.voices.delete(id);}
  }
  position(){return this.playing?{...(this.clock?.position(this.context.currentTime)??{phase:'loading',tick:0}),mode:this.mode}:{phase:'stopped',tick:0};}
  recordingReady(){return this.playing&&this.mode==='session'&&this.clock!==null&&this.context.currentTime>=this.clock.startTime;}
  async play(project,onPosition=()=>{},{mode='session'}={}){
    if(!['session','audition'].includes(mode))throw new Error('Invalid playback mode');
    const settings=transportSettings(project.transport);if(mode==='audition'){settings.countInBars=0;settings.metronome=false;}
    this.stop();const generation=++this.generation;this.onPosition=onPosition;this.playing=true;this.phase='loading';this.clock=null;this.mode=mode;this.project=structuredClone(project);this.mutes=new Map(project.tracks.map(t=>[t.id,t.muted]));
    this.setMetronome(settings.metronome,settings.clickLevel);onPosition(0,this.position());
    try{
      await this.ready();if(generation!==this.generation)return false;
      const assets=[...new Set(this.project.tracks.filter(t=>t.assetId&&!t.muted).map(t=>t.assetId))];await Promise.all(assets.map(id=>this.loadAsset(id)));
      if(generation!==this.generation)return false;
      const clock=this.clock=transportClock(this.project,this.context.currentTime+.06,settings.countInBars);this.startTime=clock.startTime;
      const {secondsPerTick,loopTicks,loopSeconds,beatSeconds}=clock,events=[];
      for(const t of this.project.tracks){
        if(t.kind==='audio'&&t.assetId){const tick=t.audio?.startTick??0;if(tick<loopTicks)events.push({tick,track:t,audio:true});}
        else for(const n of t.notes)if(n.startTick<loopTicks)events.push({tick:n.startTick,track:t,note:n});
      }
      events.sort((a,b)=>a.tick-b.tick);let loop=0,index=0,countClick=0,songClick=0;
      const schedule=()=>{
        if(!this.playing||generation!==this.generation)return;
        const now=this.context.currentTime;
        // Resume at the current loop after a throttled tab, without replaying a backlog.
        const currentLoop=Math.max(0,Math.floor((now-this.startTime)/loopSeconds));if(loop<currentLoop){loop=currentLoop;index=0;}
        while(events.length){
          const e=events[index],at=this.startTime+loop*loopSeconds+e.tick*secondsPerTick;
          if(at>now+.13)break;
          if(at>=now-.05&&!this.mutes.get(e.track.id)){
            if(e.audio){
              const buffer=this.buffers.get(e.track.assetId),view=clipWindow(e.track.audio??{durationSeconds:buffer?.duration}),startAt=Math.max(now,at),offset=view.trimStartSeconds+Math.max(0,now-at);
              const stopAt=Math.min(at+view.durationSeconds,at+(buffer?.duration??0)-view.trimStartSeconds,this.startTime+(loop+1)*loopSeconds);
              if(buffer&&stopAt>startAt){
                const src=this.context.createBufferSource();src.buffer=buffer;const g=this.context.createGain();g.gain.value=e.track.gain;src.connect(g);g.connect(this.master);
                if(view.fadeInSeconds||view.fadeOutSeconds){const points=clipGainPoints(e.track.audio,Math.min(view.durationSeconds,stopAt-at),startAt-at);g.gain.setValueAtTime(points[0].level*e.track.gain,startAt);for(const point of points.slice(1))g.gain.linearRampToValueAtTime(point.level*e.track.gain,at+point.seconds);}
                src.start(startAt,offset);src.stop(stopAt);const id=crypto.randomUUID();this.voices.set(id,{source:src,gain:g,trackId:e.track.id});src.onended=()=>{this.voices.delete(id);src.disconnect();g.disconnect();};
              }
            }
            else this.voice(e.note.pitch,e.note.velocity16/65535,e.track.instrument,Math.max(now,at),Math.min(e.note.durationTicks*secondsPerTick,loopSeconds),this.buffers.get(e.track.assetId),e.track.sampleRoot??60,e.track.gain,e.track.id);
          }
          index++;if(index===events.length){index=0;loop++;}
        }
        countClick=Math.max(countClick,Math.ceil((now-clock.anchor)/beatSeconds),0);
        while(countClick<clock.countInBeats&&clock.anchor+countClick*beatSeconds<=now+.13){this.click(clock.anchor+countClick*beatSeconds,countClick%4===0,'count-in');countClick++;}
        songClick=Math.max(songClick,Math.ceil((now-this.startTime)/beatSeconds),0);
        if(this.clickEnabled)while(this.startTime+songClick*beatSeconds<=now+.13){this.click(this.startTime+songClick*beatSeconds,songClick%4===0,'song');songClick++;}
        const position=this.position();this.phase=position.phase;onPosition(position.tick,position);this.timer=setTimeout(schedule,25);
      };schedule();return true;
    }catch(error){if(generation!==this.generation)return false;this.stop();throw error;}
  }
  tick(){return this.position().tick;}
  setTrackMuted(id,muted){if(this.mutes)this.mutes.set(id,muted);if(muted&&this.context)for(const voice of this.voices.values())if(voice.trackId===id){voice.gain.gain.cancelScheduledValues(this.context.currentTime);voice.gain.gain.setTargetAtTime(0,this.context.currentTime,0.005);}}
  stop(){this.generation++;this.playing=false;this.phase='stopped';this.clock=null;clearTimeout(this.timer);for(const {source,gain}of this.voices.values()){try{gain.gain.cancelScheduledValues(0);gain.gain.value=0;source.stop();}catch{}}this.voices.clear();this.onPosition(0,{phase:'stopped',tick:0});}
}
export function bufferWAV(buffer,{startFrame=0,endFrame=buffer.length}={}){
  if(!Number.isInteger(startFrame)||!Number.isInteger(endFrame)||startFrame<0||endFrame>buffer.length||endFrame<=startFrame)throw new Error('Invalid analysis audio window');
  const length=endFrame-startFrame,bytes=new ArrayBuffer(44+length*2),view=new DataView(bytes),text=(at,s)=>{for(let i=0;i<s.length;i++)view.setUint8(at+i,s.charCodeAt(i));};
  text(0,'RIFF');view.setUint32(4,36+length*2,true);text(8,'WAVEfmt ');view.setUint32(16,16,true);view.setUint16(20,1,true);view.setUint16(22,1,true);view.setUint32(24,buffer.sampleRate,true);view.setUint32(28,buffer.sampleRate*2,true);view.setUint16(32,2,true);view.setUint16(34,16,true);text(36,'data');view.setUint32(40,length*2,true);
  const channels=Array.from({length:buffer.numberOfChannels},(_,i)=>buffer.getChannelData(i));
  for(let i=0;i<length;i++){let sample=0;for(const channel of channels)sample+=channel[i+startFrame]/channels.length;view.setInt16(44+i*2,Math.round(Math.max(-1,Math.min(1,sample))*32767),true);}
  return bytes;
}
