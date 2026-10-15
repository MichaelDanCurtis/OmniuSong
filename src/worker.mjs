import { spawn } from 'node:child_process';
import { access } from 'node:fs/promises';
import { resolve, join } from 'node:path';
import { demand } from './project.mjs';

export const projectRoot=resolve(import.meta.dirname,'..');
export const basicPitchPython=join(projectRoot,'.runtime/basic-pitch',process.platform==='win32'?'Scripts/python.exe':'bin/python');
export async function basicPitchAvailable(){try{await access(basicPitchPython);return true;}catch{return false;}}
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
