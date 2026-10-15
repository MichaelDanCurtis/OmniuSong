import { demand, makeTrack, uid } from './project.mjs';
import { compose } from './composer.mjs';
import { renderSample } from './wav.mjs';
import { runBasicPitch, runAria } from './worker.mjs';
import { ariaPhrase, ariaCandidate } from './continuation.mjs';
import { join } from 'node:path';

export class PluginRegistry {
  constructor(){this.entries=new Map();}
  register(manifest,run) {
    demand(manifest.apiVersion===1,'Unsupported plugin API');
    demand(/^[a-z][a-z0-9.-]+$/.test(manifest.id),'Invalid plugin id');
    demand(!this.entries.has(manifest.id),'Plugin already registered');
    demand(['local','loopback','cloud'].includes(manifest.execution),'Invalid execution location');
    demand(Array.isArray(manifest.capabilities)&&manifest.capabilities.length>0,'Plugin needs capabilities');
    demand(typeof run==='function','Plugin needs a handler');
    this.entries.set(manifest.id,{manifest:Object.freeze(structuredClone(manifest)),run});return this;
  }
  list(){return [...this.entries.values()].map(e=>e.manifest);}
  async run(id,context){
    const p=this.entries.get(id);demand(p,'Plugin is not installed');
    demand(p.manifest.execution!=='cloud','Cloud execution is disabled in this build');
    demand(p.manifest.capabilities.includes(context.capability),'Plugin does not support this task');
    context.signal.throwIfAborted();
    return p.run(context);
  }
}
export function createPlugins(store,config={}) {
  const registry=new PluginRegistry();
  registry.register({apiVersion:1,id:'local.sketch',name:'Pocket composer',execution:'local',capabilities:['compose'],ai:false,version:'0.1.0',description:'Seeded musical rules. Creates editable note tracks.'},async({input,project})=>{
    demand(Number.isInteger(input.seed)&&input.seed>=0&&input.seed<=0xffffffff,'Seed must be a 32 bit unsigned integer');
    demand(Number.isFinite(input.density)&&input.density>=0&&input.density<=1,'Density must be 0–1');
    return compose({bars:project.bars,seed:input.seed,density:input.density});
  });
  registry.register({apiVersion:1,id:'local.instrument',name:'Glass instrument lab',execution:'local',capabilities:['instrument'],ai:false,version:'0.1.0',description:'Procedural FM sample creation. Play the result on keys.'},async({input})=>{
    const bytes=renderSample({pitch:60,seconds:2,brightness:input.brightness??0.5,seed:input.seed??1});
    const assetId=await store.putAsset(bytes);
    const track={...makeTrack('Glass · C4 sample','sample'),assetId,sampleRoot:60};
    return {tracks:[track],assetId,summary:'A playable procedural FM sample at C4. No AI model was used.'};
  });
  registry.register({apiVersion:1,id:'local.lyric-cards',name:'Lyric cards',execution:'local',capabilities:['lyrics'],ai:false,version:'0.1.0',description:'A writing prompt assembled from fixed lines. Editable, and clearly labeled.'},async({input})=>{
    const theme=String(input.prompt??'the lights of home').slice(0,200).replace(/[\r\n]/g,' ');
    return {lyrics:`[Verse]\nI kept a little room for ${theme}\nA quiet place to let the daylight in\n\n[Chorus]\nWe can start again, we can start again\nLeave a little light for where we've been`,summary:'A fixed writing template. No AI model was used.'};
  });
  registry.register({apiVersion:1,id:'ollama.lyrics',name:'Ollama lyric writer',execution:'loopback',capabilities:['lyrics'],ai:true,version:'0.1.0',description:'Runs a model already installed in local Ollama. Requires OMNIUSONG_OLLAMA_MODEL.'},async({input,signal})=>{
    demand(config.ollamaModel,'Set OMNIUSONG_OLLAMA_MODEL to an installed local model before running this adapter');
    const response=await fetch('http://127.0.0.1:11434/api/generate',{method:'POST',headers:{'Content-Type':'application/json'},signal,
      body:JSON.stringify({model:config.ollamaModel,stream:false,keep_alive:'5m',prompt:`Write original song lyrics. Return lyrics only, with [Verse] and [Chorus] tags. Keep the chorus singable. Theme: ${String(input.prompt??'').slice(0,4000)}`})});
    demand(response.ok,`Local Ollama returned ${response.status}`);
    const result=await response.json();demand(typeof result.response==='string'&&result.response.trim().length>0,'Ollama returned no lyrics');
    demand(result.response.length<=50000,'Lyrics response is too long');
    return {lyrics:result.response,model:config.ollamaModel,summary:`Generated locally with ${config.ollamaModel}`};
  });
  registry.register({apiVersion:1,id:'spotify.basic-pitch',name:'Basic Pitch transcription',execution:'local',capabilities:['transcribe'],ai:true,version:'0.1.0',description:'Real local ONNX inference. Best on a single instrument or vocal.'},async({input,project,signal})=>{
    demand(typeof input.assetId==='string'&&input.assetId.endsWith('.wav'),'Transcription needs a WAV asset');
    await store.asset(input.assetId);
    const result=await runBasicPitch(join(store.root,'assets',input.assetId),project.tempo,signal);
    const track=makeTrack('Transcribed · Basic Pitch','keys',result.notes);
    track.sourceAssetId=input.sourceAssetId??input.assetId;
    track.transcription={model:result.model,modelSha256:result.modelSha256,elapsedSeconds:result.elapsedSeconds,sourceDurationSeconds:result.sourceDurationSeconds};
    return {tracks:[track],model:result.model,modelSha256:result.modelSha256,summary:`${result.notes.length} estimated notes from real local Basic Pitch inference. Original audio retained; review pitches and timing.`};
  });
  registry.register({apiVersion:1,id:'eleutherai.aria-mlx',name:'Aria piano continuation',execution:'local',capabilities:['continue'],ai:true,version:'0.1.0',description:'Real local MLX piano continuation from selected source bars. Requires the qualified Apple Silicon runtime.'},async({input,project,signal})=>{
    const {request,scope}=ariaPhrase(project,input);
    const result=await runAria(request,signal);
    return ariaCandidate(project,scope,result);
  });
  return registry;
}
