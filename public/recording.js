export const AUDIO_LIMIT_BYTES=30*1024*1024;
export const AUDIO_TYPES={wav:'audio/wav',webm:'audio/webm',mp4:'audio/mp4',m4a:'audio/mp4',ogg:'audio/ogg',mp3:'audio/mpeg',flac:'audio/flac'};
export function audioType(name='',type=''){
  const mime=type.split(';')[0].toLowerCase(),alias={'audio/x-wav':'audio/wav','audio/mp3':'audio/mpeg','audio/x-flac':'audio/flac','audio/x-m4a':'audio/mp4'};
  const canonical=alias[mime]??mime;
  if(Object.values(AUDIO_TYPES).includes(canonical))return canonical;
  const extension=name.toLowerCase().split('.').at(-1);
  if(AUDIO_TYPES[extension])return AUDIO_TYPES[extension];
  throw new Error('Choose WAV, MP3, M4A, FLAC, Ogg, or WebM audio');
}
// These clocks locate the recorder start command, not the first captured sample.
export function validateRecordingTiming(timing){
  const finite=(n,min,max)=>Number.isFinite(n)&&n>=min&&n<=max;
  if(!timing||timing.version!==1||timing.method!=='browser-recorder-start-call'||timing.calibration!=='unmeasured'||
    !finite(timing.captureAudioTimeSeconds,0,1e12)||!finite(timing.songStartAudioTimeSeconds,0,1e12)||
    !finite(timing.tempo,30,300)||timing.ppq!==960||!Number.isInteger(timing.loopTicks)||timing.loopTicks<3840||timing.loopTicks>128*3840||timing.loopTicks%3840!==0)throw new Error('Invalid recording timing estimate');
  return timing;
}
export function recordingTiming(project,clock,audioTime){
  if(!clock)throw new Error('The backing song did not start');
  return validateRecordingTiming({version:1,method:'browser-recorder-start-call',calibration:'unmeasured',captureAudioTimeSeconds:audioTime,songStartAudioTimeSeconds:clock.startTime,tempo:project.tempo,ppq:project.ppq,loopTicks:clock.loopTicks});
}
export function recordingPlacement(timing,durationSeconds){
  validateRecordingTiming(timing);
  if(!Number.isFinite(durationSeconds)||durationSeconds<.001||durationSeconds>86400)throw new Error('Invalid recording duration');
  const secondsPerTick=60/timing.tempo/timing.ppq,elapsed=timing.captureAudioTimeSeconds-timing.songStartAudioTimeSeconds;
  const startTick=Math.max(0,Math.round(elapsed/secondsPerTick)),trimStartSeconds=Math.max(0,-elapsed);
  if(durationSeconds-trimStartSeconds<.001)return {status:'ended-before-song',startTick:0,trimStartSeconds:0};
  if(startTick+(durationSeconds-trimStartSeconds)/secondsPerTick>128*3840)return {status:'outside-timeline',startTick:0,trimStartSeconds:0};
  return {status:'estimated',startTick,trimStartSeconds};
}
export function takePlacement(context){
  if(context.recordingTiming){const {status,...placement}=recordingPlacement(context.recordingTiming,context.durationSeconds);return {...placement,recordingTiming:{...structuredClone(context.recordingTiming),status}};}
  const startTick=context.startTick??0;
  if(!Number.isInteger(startTick)||startTick<0||startTick>=128*3840)throw new Error('Choose a starting bar from 1 to 128');
  return {startTick};
}
export class TakeCapture {
  constructor({getStream,createRecorder,persist,prepare=null,atStart=()=>({}),onState=()=>{},onError=()=>{},now=()=>performance.now(),setTimer=setTimeout,clearTimer=clearTimeout,maxMilliseconds=120000,maxBytes=AUDIO_LIMIT_BYTES}){
    Object.assign(this,{getStream,createRecorder,persist,prepare,atStart,onState,onError,now,setTimer,clearTimer,maxMilliseconds,maxBytes});this.phase='idle';this.take=null;
  }
  get busy(){return this.phase!=='idle';}
  get blob(){return this.take?.blob;}
  setPhase(phase){this.phase=phase;this.onState({phase,context:this.take?.context,hasBlob:Boolean(this.blob),error:this.take?.error});}
  stopStream(take){if(!take.stream||take.streamStopped)return;take.streamStopped=true;for(const track of take.stream.getTracks())track.stop();}
  release(take){take.controller?.abort();this.stopStream(take);}
  reset(take){if(this.take!==take)return;this.take=null;this.setPhase('idle');}
  async start(context){
    if(this.busy)throw new Error('Finish the current take first');
    const take={context:structuredClone(context),chunks:[],bytes:0,controller:new AbortController()};this.take=take;this.setPhase('requesting');
    try{
      take.stream=await this.getStream(take.context,take.controller.signal);
      if(this.take!==take){this.stopStream(take);return;}
      const recorder=this.createRecorder(take.stream);take.recorder=recorder;
      if(this.prepare){this.setPhase('preparing');await this.prepare(take.context,take.controller.signal);if(this.take!==take){this.stopStream(take);return;}}
      recorder.ondataavailable=event=>{if(this.take===take&&!take.finalizing&&event.data.size){take.chunks.push(event.data);take.bytes+=event.data.size;if(take.bytes>=this.maxBytes&&this.phase==='recording')this.stop();}};
      recorder.onstop=()=>this.finalize(take);
      recorder.onerror=event=>{
        if(this.take!==take)return;
        take.warning=event.error?.message??'Microphone recording was interrupted';
        // An error can set the recorder inactive before its final data/stop events.
        // Wait for those events rather than finalizing an incomplete container.
        if(this.phase==='recording'&&recorder.state==='inactive'){take.stoppedAt=this.now();this.setPhase('stopping');}
        else this.stop();
      };
      Object.assign(take.context,this.atStart(take.context));take.startedAt=this.now();take.context.startedAt=new Date().toISOString();recorder.start(1000);this.setPhase('recording');
      take.timer=this.setTimer(()=>{if(this.take===take)this.stop();},this.maxMilliseconds);
      for(const track of take.stream.getTracks())track.addEventListener?.('ended',()=>{if(this.take===take&&this.phase==='recording'){take.warning='Microphone disconnected; retained the available recording';this.stop();}},{once:true});
    }catch(error){const current=this.take===take;this.release(take);this.reset(take);if(current)this.onError(error);}
  }
  stop(){
    const take=this.take;if(!take)return;
    if(['requesting','preparing'].includes(this.phase)){this.release(take);this.reset(take);return;}
    if(this.phase!=='recording')return;
    take.stoppedAt=this.now();this.setPhase('stopping');
    try{if(take.recorder.state==='inactive')this.finalize(take);else take.recorder.stop();}
    catch(error){take.warning=error.message;this.finalize(take);}
  }
  async finalize(take){
    if(this.take!==take||take.finalizing)return;take.finalizing=true;
    this.clearTimer(take.timer);this.release(take);
    take.context.durationSeconds=Math.max(.001,((take.stoppedAt??this.now())-take.startedAt)/1000);
    take.blob=new Blob(take.chunks,{type:take.recorder.mimeType||take.chunks[0]?.type||'audio/webm'});take.chunks=[];
    if(!take.blob.size){this.reset(take);this.onError(new Error(take.warning??'The microphone returned no audio'));return;}
    if(take.warning)take.context.interruption=take.warning;
    await this.save(take);
  }
  async import(blob,context){
    if(this.busy)throw new Error('Finish the current take first');
    if(!blob.size||blob.size>this.maxBytes)throw new Error('Choose audio between 1 byte and 30 MiB');
    const take={context:{...context,origin:'import'},blob};this.take=take;await this.save(take);
  }
  async save(take){
    this.setPhase('saving');
    try{await this.persist(take.blob,take.context);const warning=take.context.interruption;this.reset(take);if(warning)this.onError(new Error(warning));}
    catch(error){take.error=error.message;this.setPhase('failed');this.onError(error);}
  }
  async retry(){if(this.phase!=='failed'||!this.blob)return;await this.save(this.take);}
  cancelImport(){if(this.phase==='failed'&&this.take?.context.origin==='import')this.reset(this.take);}
}
