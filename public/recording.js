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
export class TakeCapture {
  constructor({getStream,createRecorder,persist,onState=()=>{},onError=()=>{},now=()=>performance.now(),setTimer=setTimeout,clearTimer=clearTimeout,maxMilliseconds=120000,maxBytes=AUDIO_LIMIT_BYTES}){
    Object.assign(this,{getStream,createRecorder,persist,onState,onError,now,setTimer,clearTimer,maxMilliseconds,maxBytes});this.phase='idle';this.take=null;
  }
  get busy(){return this.phase!=='idle';}
  get blob(){return this.take?.blob;}
  setPhase(phase){this.phase=phase;this.onState({phase,context:this.take?.context,hasBlob:Boolean(this.blob),error:this.take?.error});}
  stopStream(take){for(const track of take.stream?.getTracks()??[])track.stop();}
  reset(take){if(this.take!==take)return;this.take=null;this.setPhase('idle');}
  async start(context){
    if(this.busy)throw new Error('Finish the current take first');
    const take={context:{...context},chunks:[],bytes:0};this.take=take;this.setPhase('requesting');
    try{
      take.stream=await this.getStream();
      if(this.take!==take){this.stopStream(take);return;}
      const recorder=this.createRecorder(take.stream);take.recorder=recorder;
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
      take.startedAt=this.now();take.context.startedAt=new Date().toISOString();recorder.start(1000);this.setPhase('recording');
      take.timer=this.setTimer(()=>{if(this.take===take)this.stop();},this.maxMilliseconds);
      for(const track of take.stream.getTracks())track.addEventListener?.('ended',()=>{if(this.take===take&&this.phase==='recording'){take.warning='Microphone disconnected; retained the available recording';this.stop();}},{once:true});
    }catch(error){const current=this.take===take;this.stopStream(take);this.reset(take);if(current)this.onError(error);}
  }
  stop(){
    const take=this.take;if(!take)return;
    if(this.phase==='requesting'){this.reset(take);return;}
    if(this.phase!=='recording')return;
    take.stoppedAt=this.now();this.setPhase('stopping');
    try{if(take.recorder.state==='inactive')this.finalize(take);else take.recorder.stop();}
    catch(error){take.warning=error.message;this.finalize(take);}
  }
  async finalize(take){
    if(this.take!==take||take.finalizing)return;take.finalizing=true;
    this.clearTimer(take.timer);this.stopStream(take);
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
