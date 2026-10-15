import { access, readFile, stat, writeFile, mkdtemp, rm } from 'node:fs/promises';
import { spawn } from 'node:child_process';
import { createHash } from 'node:crypto';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { demand } from './project.mjs';

const projectRoot=fileURLToPath(new URL('../',import.meta.url));
export const magentaPython=join(projectRoot,'.runtime/magenta/bin/python');
const manifestPath=join(projectRoot,'workers/magenta-assets.json');
export const magentaManifest=JSON.parse(await readFile(manifestPath,'utf8'));
export async function magentaInstalled(){
  if(process.platform!=='darwin'||process.arch!=='arm64')return false;
  try{await Promise.all([access(magentaPython),...magentaManifest.files.map(f=>access(join(projectRoot,'.runtime/magenta-model',f.path)))]);return true;}catch{return false;}
}
export async function runMagenta(request,signal){
  signal.throwIfAborted();
  demand(await magentaInstalled(),'Magenta Small is not installed. See scripts/setup-magenta.sh.');
  const bytes=JSON.stringify(request);demand(Buffer.byteLength(bytes)<=128000,'Magenta request exceeds its input limit');
  const directory=await mkdtemp(join(tmpdir(),'omniusong-magenta-'));
  try{
    const input=join(directory,'request.json'),output=join(directory,'answer.wav');await writeFile(input,bytes,{mode:0o600});
    const result=await new Promise((resolve,reject)=>{
      const boundedSignal=AbortSignal.any([signal,AbortSignal.timeout(120000)]);
      const child=spawn(magentaPython,[join(projectRoot,'workers/magenta_worker.py'),input,output],{cwd:projectRoot,shell:false,stdio:['ignore','pipe','pipe'],signal:boundedSignal,
        env:{PATH:process.env.PATH,HOME:process.env.HOME,TMPDIR:process.env.TMPDIR,PYTHONNOUSERSITE:'1',HF_HUB_OFFLINE:'1',HF_HUB_DISABLE_IMPLICIT_TOKEN:'1',OMP_NUM_THREADS:'2',OPENBLAS_NUM_THREADS:'2',VECLIB_MAXIMUM_THREADS:'2'}});
      let total=0,stderr='';const chunks=[];
      child.stdout.on('data',data=>{total+=data.length;if(total>1024*1024){child.kill();reject(new Error('Magenta result exceeded its output limit'));}else chunks.push(data);});
      child.stderr.on('data',data=>{stderr=(stderr+data.toString()).slice(-4000);});child.on('error',reject);
      child.on('close',code=>{
        if(code!==0){reject(new Error(boundedSignal.aborted?'Magenta canceled or timed out':'Magenta worker failed: '+stderr.slice(-1200)));return;}
        try{resolve(JSON.parse(Buffer.concat(chunks).toString()));}catch(error){reject(error);}
      });
    });
    signal.throwIfAborted();
    demand(result.backend==='mlx-exported-function'&&result.modelRevision===magentaManifest.revision&&result.sourceRevision===magentaManifest.sourceRevision,'Unqualified Magenta result');
    demand(result.modelSha256===magentaManifest.files.find(f=>f.path==='models/mrt2_small/mrt2_small.mlxfn').sha256,'Magenta exported model identity mismatch');
    demand(result.assetManifestSha256===createHash('sha256').update(await readFile(manifestPath)).digest('hex'),'Magenta asset manifest identity mismatch');
    demand(result.durationSeconds===request.durationSeconds&&Number.isInteger(result.durationSeconds)&&result.durationSeconds>=1&&result.durationSeconds<=8,'Invalid Magenta duration');
    demand(result.sampleRate===48000&&result.channels===2&&result.sampleFrames===48000*result.durationSeconds,'Invalid Magenta audio format');
    const size=(await stat(output)).size;demand(size===44+result.sampleFrames*4&&size===result.audioBytes,'Invalid Magenta audio size');
    const audio=await readFile(output);
    demand(audio.toString('ascii',0,4)==='RIFF'&&audio.readUInt32LE(4)+8===size&&audio.toString('ascii',8,16)==='WAVEfmt '&&audio.readUInt32LE(16)===16&&audio.readUInt16LE(20)===1&&audio.readUInt16LE(22)===2&&audio.readUInt32LE(24)===48000&&audio.readUInt32LE(28)===192000&&audio.readUInt16LE(32)===4&&audio.readUInt16LE(34)===16&&audio.toString('ascii',36,40)==='data'&&audio.readUInt32LE(40)===size-44,'Invalid Magenta PCM WAV');
    demand(createHash('sha256').update(audio).digest('hex')===result.audioSha256,'Magenta audio checksum failed');
    demand(Number.isFinite(result.rmsAmplitude)&&result.rmsAmplitude>1e-6&&Number.isFinite(result.peakAmplitude)&&result.peakAmplitude<=1,'Magenta returned invalid amplitude evidence');
    return {result,audio};
  }finally{await rm(directory,{recursive:true,force:true});}
}
