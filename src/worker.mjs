import { spawn } from 'node:child_process';
import { access, mkdtemp, writeFile, rm } from 'node:fs/promises';
import { resolve, join } from 'node:path';
import { tmpdir } from 'node:os';
import { demand, validateNote } from './project.mjs';

export const projectRoot=resolve(import.meta.dirname,'..');
export const basicPitchPython=join(projectRoot,'.runtime/basic-pitch',process.platform==='win32'?'Scripts/python.exe':'bin/python');
export async function basicPitchAvailable(){try{await access(basicPitchPython);return true;}catch{return false;}}
export const ariaPython=join(projectRoot,'.runtime/aria',process.platform==='win32'?'Scripts/python.exe':'bin/python');
export async function ariaInstalled(){
  if(process.platform!=='darwin'||process.arch!=='arm64')return false;
  try{await Promise.all([access(ariaPython),access(join(projectRoot,'.runtime/aria-model/model-gen.safetensors'))]);return true;}catch{return false;}
}
export async function runAria(request,signal){
  demand(await ariaInstalled(),'Aria MLX is not installed. See scripts/setup-aria.sh.');
  signal.throwIfAborted();
  const bytes=JSON.stringify(request);demand(Buffer.byteLength(bytes)<=128000,'Aria prompt exceeds its input limit');
  const directory=await mkdtemp(join(tmpdir(),'omniusong-aria-'));
  try{
    const path=join(directory,'phrase.json');await writeFile(path,bytes,{mode:0o600});
    return await new Promise((resolveResult,reject)=>{
      const boundedSignal=AbortSignal.any([signal,AbortSignal.timeout(120000)]);
      const child=spawn(ariaPython,[join(projectRoot,'workers/aria_worker.py'),path],{cwd:projectRoot,stdio:['ignore','pipe','pipe'],shell:false,signal:boundedSignal,
        env:{PATH:process.env.PATH,HOME:process.env.HOME,TMPDIR:process.env.TMPDIR,PYTHONNOUSERSITE:'1',OMP_NUM_THREADS:'2',VECLIB_MAXIMUM_THREADS:'2'}});
      const stdout=[];let total=0,stderr='';
      child.stdout.on('data',data=>{total+=data.length;if(total>1024*1024){child.kill();reject(new Error('Aria result exceeded its output limit'));}else stdout.push(data);});
      child.stderr.on('data',data=>{stderr=(stderr+data.toString()).slice(-4000);});
      child.on('error',reject);
      child.on('close',code=>{
        if(code!==0){reject(new Error(boundedSignal.aborted?'Aria canceled or timed out':'Aria worker failed: '+stderr.slice(-1200)));return;}
        try{
          const result=JSON.parse(Buffer.concat(stdout).toString());
          demand(result.backend==='mlx'&&result.modelSha256==='5e6b07ee2680f0044deea30da613d89b12e3e30d012d1812b388d6b8d8306c90','Unqualified Aria result');
          demand(Array.isArray(result.notes)&&result.notes.length>0&&result.notes.length<=256,'Invalid Aria notes');
          for(const note of result.notes)validateNote(note);resolveResult(result);
        }catch(error){reject(error);}
      });
    });
  }finally{await rm(directory,{recursive:true,force:true});}
}
export async function runBasicPitch(assetPath,tempo,signal){
  demand(await basicPitchAvailable(),'Basic Pitch is not installed. Run scripts/setup-basic-pitch.sh in this project.');
  return new Promise((resolveResult,reject)=>{
    const child=spawn(basicPitchPython,[join(projectRoot,'workers/basic_pitch_worker.py'),resolve(assetPath),String(tempo)],{cwd:projectRoot,stdio:['ignore','pipe','pipe'],shell:false,signal});
    const stdout=[];let total=0,stderr='';
    child.stdout.on('data',data=>{total+=data.length;if(total>8*1024*1024){child.kill();reject(new Error('Transcription result exceeded the output limit'));}else stdout.push(data);});
    child.stderr.on('data',data=>{stderr=(stderr+data.toString()).slice(-4000);});
    child.on('error',reject);
    child.on('close',code=>{
      if(code!==0){reject(new Error(signal.aborted?'Transcription canceled':'Basic Pitch worker failed: '+stderr.slice(-1200)));return;}
      try{const result=JSON.parse(Buffer.concat(stdout).toString());demand(Array.isArray(result.notes)&&result.notes.length<=20000,'Invalid transcription result');resolveResult(result);}catch(error){reject(error);}
    });
  });
}
