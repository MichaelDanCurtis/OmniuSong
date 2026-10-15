import { clipWindow, clipGainPoints } from './clip-edit.js';
const demand=(condition,message)=>{if(!condition)throw new Error(message);};
export function waveformPeaks(buffer,bins=256){
  const {sampleRate,length,numberOfChannels}=buffer;
  demand(Number.isInteger(sampleRate)&&sampleRate>=8000&&sampleRate<=384000&&Number.isInteger(length)&&length>0&&length<=sampleRate*120&&Number.isInteger(numberOfChannels)&&numberOfChannels>=1&&numberOfChannels<=32,'Waveform requires decoded audio up to two minutes and 32 channels');
  demand(length*numberOfChannels<=32*1024*1024,'Waveform exceeds the 32-million-sample review limit');
  demand(Number.isInteger(bins)&&bins>=1&&bins<=1024,'Invalid waveform resolution');
  const channels=Array.from({length:numberOfChannels},(_,i)=>buffer.getChannelData(i));
  demand(channels.every(channel=>channel.length===length),'Incomplete decoded waveform');
  const count=Math.min(bins,length),peaks=[];
  for(let bin=0;bin<count;bin++){
    const start=Math.floor(bin*length/count),end=Math.floor((bin+1)*length/count);let min=0,max=0;
    for(const channel of channels)for(let frame=start;frame<end;frame++){const value=channel[frame];demand(Number.isFinite(value),'Decoded waveform contains non-finite samples');min=Math.min(min,value);max=Math.max(max,value);}
    peaks.push({min,max});
  }
  return {sampleRate,length,numberOfChannels,durationSeconds:length/sampleRate,peaks};
}
export function waveformWindow(audio,summary){
  const view=clipWindow(audio),decodedSeconds=summary.durationSeconds;
  demand(Number.isFinite(decodedSeconds)&&decodedSeconds>0,'Invalid decoded waveform duration');
  const axisSeconds=Math.max(audio.durationSeconds,decodedSeconds),audibleSeconds=Math.max(0,Math.min(view.durationSeconds,decodedSeconds-view.trimStartSeconds));
  return {axisSeconds,sourceIn:view.trimStartSeconds,sourceOut:view.trimEndSeconds,decodedSeconds,audibleSeconds,
    fadePoints:audibleSeconds?clipGainPoints(audio,audibleSeconds).map(point=>({seconds:point.seconds+view.trimStartSeconds,level:point.level})):[]};
}
// Selection tickets prevent a late decode from publishing into another track or session.
export class WaveformReview {
  constructor({load,onChange=()=>{}}){this.load=load;this.onChange=onChange;this.cache=new Map();this.ticket=0;this.key=null;this.assetId=null;this.expanded=false;this.loading=false;this.error=null;}
  select(key,assetId){if(this.key===key&&this.assetId===assetId)return;this.ticket++;this.key=key;this.assetId=assetId;this.expanded=false;this.loading=false;this.error=null;}
  get data(){return this.expanded?this.cache.get(this.assetId):null;}
  close(){this.ticket++;this.expanded=false;this.loading=false;this.error=null;this.onChange();}
  async open(){
    if(!this.assetId||this.loading)return;this.expanded=true;this.error=null;
    if(this.cache.has(this.assetId)){this.onChange();return;}
    const ticket=++this.ticket,assetId=this.assetId;this.loading=true;this.onChange();
    try{
      const decoded=await this.load(assetId);if(ticket!==this.ticket)return;const data=waveformPeaks(decoded);
      this.cache.set(assetId,data);while(this.cache.size>8)this.cache.delete(this.cache.keys().next().value);
    }catch(error){if(ticket===this.ticket)this.error=error.message;}
    finally{if(ticket===this.ticket){this.loading=false;this.onChange();}}
  }
}
