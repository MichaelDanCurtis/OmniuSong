import { gzip, gunzip } from 'node:zlib';
import { promisify } from 'node:util';
import { createHash } from 'node:crypto';
import { ASSET_ID, copy, demand, uid, validateProject } from './project.mjs';

const compress=promisify(gzip),decompress=promisify(gunzip);
export const ARCHIVE_MIME='application/vnd.omniusong.project+gzip';
export const ARCHIVE_LIMITS=Object.freeze({compressedBytes:48*1024*1024,jsonBytes:72*1024*1024,assetBytes:30*1024*1024,totalAssetBytes:48*1024*1024,assets:128,projectBytes:1_900_000});
const digest=bytes=>createHash('sha256').update(bytes).digest('hex');
function archiveError(message,status=422){const error=new Error(message);error.status=status;return error;}
function requireLimit(condition,message){if(!condition)throw archiveError(message,413);}

export function referencedAssets(project){
  const refs=new Set();let visited=0;
  function walk(value,key='',depth=0){
    demand(depth<=64&&++visited<=250000,'Project metadata is too deeply nested or complex');
    if(/assetid$/i.test(key)&&value!=null&&value!=='')demand(typeof value==='string'&&ASSET_ID.test(value),`Invalid asset reference in ${key}`);
    if(typeof value==='string'){
      if(ASSET_ID.test(value))refs.add(value);
      else if(/assetid$/i.test(key)&&value!=='')throw archiveError(`Invalid asset reference in ${key}`);
    }else if(value&&typeof value==='object'){
      if(Array.isArray(value))for(const entry of value)walk(entry,'',depth+1);
      else for(const [name,entry]of Object.entries(value))walk(entry,name,depth+1);
    }else if(/assetid$/i.test(key)&&value!=null)throw archiveError(`Invalid asset reference in ${key}`);
  }
  walk(project);requireLimit(refs.size<=ARCHIVE_LIMITS.assets,'Project contains too many audio assets for this archive version');
  return [...refs].sort();
}

export async function exportArchive(store,project){
  validateProject(project);
  requireLimit(Buffer.byteLength(JSON.stringify(project))<=ARCHIVE_LIMITS.projectBytes,'Project metadata exceeds the archive limit');
  const assets=[];let total=0;
  for(const id of referencedAssets(project)){
    let bytes;try{bytes=await store.asset(id);}catch(error){throw archiveError(error.code==='ENOENT'?`Cannot back up this project: audio asset ${id} is missing`:error.message);}
    requireLimit(bytes.length>0&&bytes.length<=ARCHIVE_LIMITS.assetBytes,'An audio asset exceeds the archive limit');
    total+=bytes.length;requireLimit(total<=ARCHIVE_LIMITS.totalAssetBytes,'Project audio exceeds the 48 MiB archive limit');
    assets.push({id,byteLength:bytes.length,encoding:'base64',data:bytes.toString('base64')});
  }
  const archive={format:'omniusong-project-archive',version:1,exportedAt:new Date().toISOString(),project:copy(project),assets};
  const json=Buffer.from(JSON.stringify(archive));requireLimit(json.length<=ARCHIVE_LIMITS.jsonBytes,'Archive metadata exceeds its limit');
  const bytes=await compress(json,{level:6});requireLimit(bytes.length<=ARCHIVE_LIMITS.compressedBytes,'Compressed archive exceeds 48 MiB');
  return bytes;
}

export async function inspectArchive(bytes){
  requireLimit(bytes.length<=ARCHIVE_LIMITS.compressedBytes,'Archive file exceeds 48 MiB');
  if(bytes[0]!==0x1f||bytes[1]!==0x8b)throw archiveError('Choose an OmniuSong .omniusong backup file');
  let decoded;
  try{decoded=await decompress(bytes,{maxOutputLength:ARCHIVE_LIMITS.jsonBytes});}catch(error){throw archiveError(error.code==='ERR_BUFFER_TOO_LARGE'?'Expanded archive exceeds 72 MiB':'The backup is damaged or is not a valid gzip file');}
  let archive;try{archive=JSON.parse(decoded.toString('utf8'));}catch{throw archiveError('The backup contains invalid project JSON');}
  demand(archive?.format==='omniusong-project-archive'&&archive.version===1,'Unsupported OmniuSong archive version');
  demand(typeof archive.exportedAt==='string'&&Number.isFinite(Date.parse(archive.exportedAt)),'Invalid backup timestamp');
  validateProject(archive.project);
  requireLimit(Buffer.byteLength(JSON.stringify(archive.project))<=ARCHIVE_LIMITS.projectBytes,'Project metadata exceeds the archive limit');
  demand(Array.isArray(archive.assets),'Archive is missing its audio manifest');
  requireLimit(archive.assets.length<=ARCHIVE_LIMITS.assets,'Archive contains too many audio assets');
  const refs=new Set(referencedAssets(archive.project)),seen=new Set(),assets=[];let total=0;
  for(const entry of archive.assets){
    demand(entry&&typeof entry.id==='string'&&ASSET_ID.test(entry.id),'Invalid archive asset id');
    demand(!seen.has(entry.id),'Duplicate archive audio asset');seen.add(entry.id);
    demand(refs.has(entry.id),'Archive includes audio outside the project');
    demand(entry.encoding==='base64'&&typeof entry.data==='string','Invalid audio encoding');
    requireLimit(Number.isSafeInteger(entry.byteLength)&&entry.byteLength>0&&entry.byteLength<=ARCHIVE_LIMITS.assetBytes,'Invalid or oversized audio asset');
    demand(entry.data.length===Math.ceil(entry.byteLength/3)*4,'Audio length does not match the manifest');
    total+=entry.byteLength;requireLimit(total<=ARCHIVE_LIMITS.totalAssetBytes,'Project audio exceeds 48 MiB');
    const data=Buffer.from(entry.data,'base64');
    demand(data.length===entry.byteLength&&data.toString('base64')===entry.data,'Audio is not canonical base64');
    demand(digest(data)===entry.id.split('.')[0],'Audio checksum does not match the manifest');
    assets.push({id:entry.id,bytes:data});
  }
  demand(seen.size===refs.size,'Backup is incomplete: referenced audio is missing');
  return {project:archive.project,assets,exportedAt:archive.exportedAt};
}

export async function importArchive(store,bytes){
  // Validate the complete bundle before writing any audio. Publish the new
  // project last, so interrupted imports cannot expose an incomplete session.
  const archive=await inspectArchive(bytes);
  for(const asset of archive.assets)await store.putAsset(asset.bytes,asset.id.split('.')[1]);
  const project=copy(archive.project);
  project.importedFrom={projectId:project.id,revision:project.revision,exportedAt:archive.exportedAt,restoredAt:new Date().toISOString(),previous:project.importedFrom??null};
  project.id=uid();project.revision=0;project.name=project.name.slice(0,149)+' (restored)';
  return {project:await store.saveProject(project,null),assetCount:archive.assets.length,archiveVersion:1};
}
