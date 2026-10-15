// Portable input lifetimes. Arrival timing and note recording never wait for audio startup.
export const UMP_WORD_COUNTS = [1,1,1,2,2,4,1,1,2,2,2,3,3,4,4,4];
export const CAPTURE_EVENT_LIMIT = 4096;
export function scale7to16(value) {
  if(!Number.isInteger(value)||value<0||value>127)throw new Error('Invalid MIDI 1 value');
  return (value<<9)+(value>64?((value&63)<<3)+((value&63)>>>3):0);
}
export function decodeMIDI1(bytes, group=0) {
  const [status,a,b]=bytes,kind=status&0xf0,channel=status&15;
  if(!Number.isInteger(status)||status<0x80||status>=0xf0)return null;
  const size=kind===0xc0||kind===0xd0?2:3;
  if(bytes.length!==size||Array.from(bytes).slice(1).some(v=>!Number.isInteger(v)||v<0||v>127))throw new Error('Malformed MIDI 1 message');
  const address={group,channel,protocol:'midi1'};
  if(kind===0x90&&b!==0)return {...address,kind:'on',pitch:a,velocity16:scale7to16(b)};
  if(kind===0x80||kind===0x90)return {...address,kind:'off',pitch:a,releaseVelocity16:kind===0x90?32768:scale7to16(b)};
  if(kind===0xb0)return {...address,kind:'control',controller:a,value:b,down:b>=64};
  return null;
}
export function decodeUMP(words) {
  if(!Array.isArray(words)||words.length>65536||words.some(w=>!Number.isInteger(w)||w<0||w>0xffffffff))throw new Error('Invalid UMP words');
  const packets=[];
  for(let i=0;i<words.length;){
    const w=words[i],type=w>>>28,count=UMP_WORD_COUNTS[type];
    if(i+count>words.length)throw new Error('Truncated UMP message');
    const packet={words:words.slice(i,i+count),event:null},group=(w>>>24)&15,status=(w>>>16)&255;
    if(type===2){const kind=status&0xf0;packet.event=decodeMIDI1([status,(w>>>8)&255,...(kind===0xc0||kind===0xd0?[]:[w&255])],group);}
    if(type===4){
      const kind=status>>>4,channel=status&15,pitch=(w>>>8)&127,address={group,channel,protocol:'midi2'};
      // Unlike MIDI 1, MIDI 2 Note On velocity zero is a real Note On.
      if(kind===9)packet.event={...address,kind:'on',pitch,velocity16:words[i+1]>>>16};
      if(kind===8)packet.event={...address,kind:'off',pitch,releaseVelocity16:words[i+1]>>>16};
      if(kind===11)packet.event={...address,kind:'control',controller:pitch,value:words[i+1],down:words[i+1]>=0x80000000};
    }
    packets.push(packet);i+=count;
  }
  return packets;
}
const channelKey=e=>JSON.stringify([e.sourceId,e.group??0,e.channel??0]);
const noteKey=e=>JSON.stringify([e.sourceId,e.group??0,e.channel??0,e.pitch]);
export class PerformanceInput {
  constructor({startVoice,onStart=()=>{},onEnd=()=>{},onKeyRelease=()=>{},onChange=()=>{},onError=()=>{},now=()=>performance.now(),id=()=>crypto.randomUUID(),maxVoices=256}) {
    Object.assign(this,{startVoice,onStart,onEnd,onKeyRelease,onChange,onError,now,id,maxVoices});
    this.voices=new Map();this.pedals=new Set();
  }
  noteOn(event) {
    if(this.voices.size>=this.maxVoices){this.onError(new Error('Too many held notes; use All notes off'));return;}
    const voice={...event,id:this.id(),startedAt:this.now(),keyDown:true,ended:false,release:null};
    this.voices.set(voice.id,voice);
    try{voice.record=this.onStart(voice);}catch(error){this.voices.delete(voice.id);this.onError(error);return;}
    this.onChange(this);
    Promise.resolve().then(()=>voice.ended?null:this.startVoice(voice)).then(release=>{
      if(typeof release!=='function')return;
      if(voice.ended)release();else voice.release=release;
    }).catch(error=>{if(!voice.ended)this.finish(voice,'audio-error');this.onError(error);});
    return voice.id;
  }
  noteOff(event) {
    // MIDI has no general same-pitch voice ID. Pair repeated notes FIFO within an address.
    const voice=event.voiceId?this.voices.get(event.voiceId):[...this.voices.values()].find(v=>v.keyDown&&noteKey(v)===noteKey(event));
    if(!voice||!voice.keyDown)return;
    voice.keyDown=false;voice.keyReleasedAt=this.now();voice.releaseVelocity16=event.releaseVelocity16??0;
    this.onKeyRelease(voice);
    if(!this.pedals.has(channelKey(voice)))this.finish(voice,'key-release');else this.onChange(this);
  }
  sustain(event,down) {
    const key=channelKey(event);
    if(down)this.pedals.add(key);else{
      this.pedals.delete(key);
      for(const voice of [...this.voices.values()])if(!voice.keyDown&&channelKey(voice)===key)this.finish(voice,'pedal-release');
    }
    this.onChange(this);
  }
  control(event) {
    if(event.controller===64)this.sustain(event,event.down);
    if(event.controller===120)this.releaseWhere(v=>channelKey(v)===channelKey(event),'all-sound-off');
    if(event.controller===121)this.sustain(event,false);
    if(event.controller===123)for(const v of [...this.voices.values()])if(v.keyDown&&channelKey(v)===channelKey(event))this.noteOff(v);
  }
  finish(voice,reason) {
    if(voice.ended)return;
    voice.ended=true;voice.endedAt=this.now();this.voices.delete(voice.id);
    voice.release?.();this.onEnd(voice,reason);this.onChange(this);
  }
  releaseWhere(predicate,reason) {for(const voice of [...this.voices.values()])if(predicate(voice))this.finish(voice,reason);}
  releaseSource(sourceId,reason='device-disconnected') {
    this.releaseWhere(v=>v.sourceId===sourceId,reason);
    for(const key of this.pedals)if(JSON.parse(key)[0]===sourceId)this.pedals.delete(key);
    this.onChange(this);
  }
  panic(reason='panic') {this.pedals.clear();this.releaseWhere(()=>true,reason);this.onChange(this);}
}
export function appendCapture(project,event) {
  project.inputCapture??={version:1,events:[],droppedEvents:0};
  if(project.inputCapture.events.length>=CAPTURE_EVENT_LIMIT){project.inputCapture.droppedEvents=Math.min(1e9,project.inputCapture.droppedEvents+1);return false;}
  project.inputCapture.events.push(structuredClone(event));return true;
}
