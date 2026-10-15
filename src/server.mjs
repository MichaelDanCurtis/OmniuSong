import http from 'node:http';
import { readFile } from 'node:fs/promises';
import { dirname, join, resolve, extname } from 'node:path';
import { fileURLToPath } from 'node:url';
import { Store, atomicJSON } from './storage.mjs';
import { newProject, validateProject, demand } from './project.mjs';
import { createPlugins } from './plugins.mjs';
import { JobQueue } from './jobs.mjs';
import { exportSMF, exportUMPTrace } from './midi.mjs';
import { renderProject, renderExamplePhrase } from './wav.mjs';
import { basicPitchAvailable, ariaInstalled } from './worker.mjs';
import { magentaInstalled } from './magenta.mjs';
import { exportArchive, importArchive, ARCHIVE_MIME, ARCHIVE_LIMITS } from './archive.mjs';

const root=resolve(dirname(fileURLToPath(import.meta.url)),'..');
const mime={'.html':'text/html; charset=utf-8','.js':'text/javascript; charset=utf-8','.mjs':'text/javascript; charset=utf-8','.css':'text/css; charset=utf-8','.json':'application/json; charset=utf-8','.wav':'audio/wav','.webm':'audio/webm','.mp4':'audio/mp4','.ogg':'audio/ogg','.mp3':'audio/mpeg','.flac':'audio/flac'};
export async function createServer({dataDir=process.env.OMNIUSONG_DATA_DIR??join(root,'data'),ollamaModel=process.env.OMNIUSONG_OLLAMA_MODEL}={}) {
  const store=await new Store(dataDir).init(),plugins=createPlugins(store,{ollamaModel}),queue=await new JobQueue(store,plugins).init();
  if(!(await store.list('projects')).length)await store.saveProject(newProject(),null);
  const catalog=JSON.parse(await readFile(join(root,'catalog/models.json'),'utf8'));
  const send=(res,status,value,contentType='application/json; charset=utf-8')=>{const bytes=Buffer.isBuffer(value)?value:Buffer.from(typeof value==='string'?value:JSON.stringify(value));res.writeHead(status,{'Content-Type':contentType,'Content-Length':bytes.length,'Cache-Control':'no-store'});res.end(bytes);};
  const body=async(req,limit=2e6)=>{const chunks=[];let size=0;for await(const c of req){size+=c.length;if(size>limit){const e=new Error('Upload is too large');e.status=413;throw e;}chunks.push(c);}return Buffer.concat(chunks);};
  const json=async req=>{demand(req.headers['content-type']?.startsWith('application/json'),'Expected JSON request');return JSON.parse((await body(req)).toString());};
  const server=http.createServer(async(req,res)=>{
    res.setHeader('X-Content-Type-Options','nosniff');
    res.setHeader('Content-Security-Policy',"default-src 'self'; script-src 'self'; style-src 'self' 'unsafe-inline'; img-src 'self' data:; media-src 'self' blob:; connect-src 'self'; frame-ancestors 'none'");
    res.setHeader('Referrer-Policy','no-referrer');
    try{
      const host=req.headers.host??'';demand(/^(127\.0\.0\.1|localhost):\d+$/.test(host),'Invalid local host');
      const origin=req.headers.origin;
      if(origin&&origin!==`http://${host}`){send(res,403,{error:'Cross-origin requests are not allowed'});return;}
      const url=new URL(req.url,`http://${host}`),path=url.pathname;
      if(req.method==='GET'&&path==='/api/status')return send(res,200,{name:'OmniuSong',version:'0.1.0',cloudEnabled:false,ollamaConfigured:Boolean(ollamaModel),basicPitchInstalled:await basicPitchAvailable(),ariaInstalled:await ariaInstalled(),ariaStudioControls:true,magentaInstalled:await magentaInstalled(),magentaStudioControls:true,midi2:'UMP encode and native input; physical hardware qualification pending'});
      if(req.method==='GET'&&path==='/api/catalog')return send(res,200,catalog);
      if(req.method==='GET'&&path==='/api/examples/four-tones.wav')return send(res,200,renderExamplePhrase(),'audio/wav');
      if(req.method==='GET'&&path==='/api/plugins')return send(res,200,plugins.list());
      if(path==='/api/projects'){
        if(req.method==='GET')return send(res,200,(await store.list('projects')).map(p=>({id:p.id,name:p.name,revision:p.revision,createdAt:p.createdAt,updatedAt:p.updatedAt,trackCount:p.tracks.length})));
        if(req.method==='POST'){const input=await json(req);return send(res,201,await store.saveProject(newProject(input.name),null));}
      }
      if(path==='/api/archives'&&req.method==='POST'){
        demand(req.headers['content-type']===ARCHIVE_MIME,'Expected an OmniuSong backup file');
        return send(res,201,await importArchive(store,await body(req,ARCHIVE_LIMITS.compressedBytes)));
      }
      const projectMatch=path.match(/^\/api\/projects\/([a-zA-Z0-9-]+)(?:\/(midi|ump|wav|archive))?$/);
      if(projectMatch){
        const [,id,format]=projectMatch;
        if(req.method==='GET'){
          const p=await store.read('projects',id);
          if(format==='midi'){const out=exportSMF(p);res.setHeader('X-OmniuSong-Warnings',encodeURIComponent(out.warnings.join('; ')));return send(res,200,out.bytes,'audio/midi');}
          if(format==='ump')return send(res,200,exportUMPTrace(p));
          if(format==='wav')return send(res,200,renderProject(p),'audio/wav');
          if(format==='archive')return send(res,200,await exportArchive(store,p),ARCHIVE_MIME);
          return send(res,200,p);
        }
        if(req.method==='PUT'&&!format){const input=await json(req);demand(input.project?.id===id,'Project id mismatch');validateProject(input.project);return send(res,200,await store.saveProject(input.project,input.expectedRevision));}
      }
      if(path==='/api/jobs'){
        if(req.method==='GET')return send(res,200,queue.list(url.searchParams.get('projectId')));
        if(req.method==='POST')return send(res,202,await queue.submit(await json(req)));
      }
      const jobMatch=path.match(/^\/api\/jobs\/([a-zA-Z0-9-]+)\/(cancel|accept|reject)$/);
      if(req.method==='POST'&&jobMatch){const [,id,action]=jobMatch;const input=await json(req);return send(res,200,action==='cancel'?await queue.cancel(id):await queue.decide(id,action==='accept'?'accepted':'rejected',input.expectedRevision));}
      if(path==='/api/assets'&&req.method==='POST'){
        const type=req.headers['content-type']?.split(';')[0];
        const extension={'audio/wav':'wav','audio/webm':'webm','audio/mp4':'mp4','audio/ogg':'ogg','audio/mpeg':'mp3','audio/flac':'flac'}[type];demand(extension,'Unsupported audio MIME type');
        const bytes=await body(req,30*1024*1024);demand(bytes.length>0,'Audio is empty');return send(res,201,{assetId:await store.putAsset(bytes,extension)});
      }
      const assetMatch=path.match(/^\/api\/assets\/([a-f0-9]{64}\.(wav|webm|mp4|ogg|mp3|flac))$/);
      if(req.method==='GET'&&assetMatch){
        const bytes=await store.asset(assetMatch[1]);res.setHeader('Accept-Ranges','bytes');
        if(req.headers.range){
          const match=/^bytes=(\d*)-(\d*)$/.exec(req.headers.range);let start,end;
          if(match&&(match[1]||match[2])){
            if(!match[1]){const suffix=Number(match[2]);if(Number.isSafeInteger(suffix)&&suffix>0){start=Math.max(0,bytes.length-suffix);end=bytes.length-1;}}
            else {start=Number(match[1]);end=match[2]?Number(match[2]):bytes.length-1;}
          }
          if(!Number.isSafeInteger(start)||!Number.isSafeInteger(end)||start<0||start>=bytes.length||end<start){res.setHeader('Content-Range',`bytes */${bytes.length}`);return send(res,416,{error:'Audio range is not available'});}
          end=Math.min(end,bytes.length-1);res.setHeader('Content-Range',`bytes ${start}-${end}/${bytes.length}`);
          return send(res,206,bytes.subarray(start,end+1),mime['.'+assetMatch[2]]);
        }
        return send(res,200,bytes,mime['.'+assetMatch[2]]);
      }
      if(path==='/api/selections'){
        const target=join(dataDir,'selections.json');
        if(req.method==='GET'){try{return send(res,200,JSON.parse(await readFile(target,'utf8')));}catch(e){if(e.code!=='ENOENT')throw e;return send(res,200,{});}}
        if(req.method==='PUT'){const input=await json(req);demand(input&&typeof input==='object'&&!Array.isArray(input),'Invalid selections');for(const [id,status]of Object.entries(input))demand(catalog.models.some(m=>m.id===id)&&['keep','explore','skip'].includes(status),'Invalid model selection');await atomicJSON(target,input);return send(res,200,input);}
      }
      const staticFiles={'/':'index.html','/app.js':'app.js','/audio.js':'audio.js','/input.js':'input.js','/recording.js':'recording.js','/note-view.js':'note-view.js','/note-edit.js':'note-edit.js','/clip-edit.js':'clip-edit.js','/style.css':'style.css'};
      if(req.method==='GET'&&staticFiles[path])return send(res,200,await readFile(join(root,'public',staticFiles[path])),mime[extname(staticFiles[path])]);
      send(res,404,{error:'Not found'});
    }catch(error){send(res,error.status??(error.code==='ENOENT'?404:400),{error:error.code==='ENOENT'?'Not found':error.message});}
  });
  return {server,store,queue,plugins};
}
if(process.argv[1]&&resolve(process.argv[1])===fileURLToPath(import.meta.url)){
  const {server,queue}=await createServer();const port=Number(process.env.OMNIUSONG_PORT??43178);
  server.listen(port,'127.0.0.1',()=>process.stdout.write(`OmniuSong is ready at http://127.0.0.1:${port}\n`));
  const stop=async()=>{await queue.close();server.close(()=>process.exit(0));setTimeout(()=>process.exit(0),3000).unref();};
  process.on('SIGINT',stop);process.on('SIGTERM',stop);
}
