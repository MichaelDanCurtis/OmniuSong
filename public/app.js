import { StudioAudio, bufferWAV } from './audio.js';
const $=s=>document.querySelector(s),$$=s=>[...document.querySelectorAll(s)],audio=new StudioAudio();
let project,selectedId,catalog,plugins,selections={},jobs=[],dirty=false,editSequence=0,savePromise=null,saveTimer,undoStack=[],recording=null,midiAccess;
const held=new Map(),colors=['#bfdb9f','#e5b677','#97bec1','#cd9dad','#9eb7d5','#d6cb90'];
const el=(tag,classes='',text='')=>{const e=document.createElement(tag);e.className=classes;e.textContent=text;return e;};
function toast(message,error=false){const box=$('#toast');box.textContent=message;box.classList.toggle('error',error);box.classList.remove('hidden');clearTimeout(toast.timer);toast.timer=setTimeout(()=>box.classList.add('hidden'),6000);}
async function api(path,options={}){const response=await fetch(path,{...options,headers:{'Content-Type':'application/json',...options.headers}});const result=await response.json();if(!response.ok)throw new Error(result.error??'Request failed');return result;}
const post=(path,data)=>api(path,{method:'POST',body:JSON.stringify(data)});
function track(){return project.tracks.find(t=>t.id===selectedId)??project.tracks[0];}
function remember(){undoStack.push(structuredClone(project));if(undoStack.length>40)undoStack.shift();}
function changed(render=true){dirty=true;editSequence++;$('#save-status').textContent='Unsaved changes';clearTimeout(saveTimer);saveTimer=setTimeout(()=>save().catch(e=>toast(e.message,true)),750);if(render)renderStudio();}
async function save(){
  if(savePromise){await savePromise;if(dirty)return save();return;}
  if(!dirty)return;
  const sequence=editSequence,snapshot=structuredClone(project),revision=project.revision;
  $('#save-status').textContent='Saving…';
  savePromise=api('/api/projects/'+project.id,{method:'PUT',body:JSON.stringify({project:snapshot,expectedRevision:revision})});
  try{const saved=await savePromise;project.revision=saved.revision;project.updatedAt=saved.updatedAt;if(sequence===editSequence){dirty=false;$('#save-status').textContent='Saved on this Mac';}else $('#save-status').textContent='Unsaved changes';$('#session-meta').textContent=`${project.bars} bars · ${project.tracks.reduce((s,t)=>s+t.notes.length,0)} notes · Revision ${project.revision}`;}
  finally{savePromise=null;}
}
function renderStudio(){
  $('#project-name').value=project.name;$('#tempo').value=project.tempo;$('#session-meta').textContent=`${project.bars} bars · ${project.tracks.reduce((s,t)=>s+t.notes.length,0)} notes · Revision ${project.revision}`;
  $('#track-count').textContent=`${project.tracks.length} TRACKS`;$('#selected-track').textContent=track()?.name??'Add a track';
  $('#clear-track').disabled=Boolean(track()?.locked);$('#undo').disabled=!undoStack.length;
  const arrangement=$('#arrangement');arrangement.replaceChildren();
  const ruler=el('div','bar-ruler');ruler.append(el('span','','TRACKS'));const numbers=el('div','bar-numbers');for(let b=1;b<=project.bars;b++)numbers.append(el('span','',String(b).padStart(2,'0')));ruler.append(numbers);arrangement.append(ruler);
  project.tracks.forEach((t,index)=>{
    const row=el('div','track-row'+(t.id===selectedId?' selected':''));row.style.setProperty('--track-color',colors[index%colors.length]);const label=el('div','track-label');label.append(el('span','track-dot'));
    const select=el('button','track-select',t.name);select.title=t.name;select.onclick=()=>{selectedId=t.id;renderStudio();};label.append(select);
    for(const [field,text]of [['muted','M'],['locked','L']]){const button=el('button','track-switch'+(t[field]?' enabled':''),text);button.title=(field==='muted'?'Mute ':'Lock ')+t.name;button.setAttribute('aria-label',button.title);button.setAttribute('aria-pressed',String(t[field]));button.onclick=()=>{remember();t[field]=!t[field];if(field==='muted')audio.setTrackMuted(t.id,t.muted);changed();};label.append(button);}
    if(t.kind==='notes'){const arm=el('button','track-switch'+(t.armed?' enabled':''),'R');arm.title='Arm note recording for '+t.name;arm.setAttribute('aria-label',arm.title);arm.setAttribute('aria-pressed',String(Boolean(t.armed)));arm.onclick=()=>{remember();t.armed=!t.armed;changed();};label.append(arm);}
    const lane=el('div','track-lane'+(t.notes.length||t.assetId?' has-notes':''));
    if(t.notes.length)for(const n of t.notes){const note=el('span','mini-note');note.style.left=(n.startTick/(project.bars*3840)*100)+'%';note.style.width=Math.max(.4,n.durationTicks/(project.bars*3840)*100)+'%';note.style.top=(5+(n.pitch%12)*2)+'px';lane.append(note);}
    else lane.append(el('span','empty-lane',t.assetId?(t.kind==='audio'?'◉ Original vocal take':'◈ Playable sample'):'Your next idea goes here'));
    row.append(label,lane);arrangement.append(row);
  });
  renderRoll();renderTakes();
  if(document.activeElement!==$('#lyric-editor'))$('#lyric-editor').value=project.lyrics;
  $('#lock-lyrics').checked=Boolean(project.lyricsLocked);$('#lyric-editor').disabled=Boolean(project.lyricsLocked);
}
function renderRoll(){
  const grid=$('#piano-roll');grid.replaceChildren();const t=track();const total=project.bars*3840,step=480,columns=32;
  grid.style.gridTemplateColumns=`28px repeat(${columns}, minmax(4px,1fr))`;
  const top=t?.instrument==='drums'?60:t?.instrument==='bass'?48:72;
  for(let pitch=top;pitch>=top-24;pitch--){
    grid.append(el('span','pitch-name',pitch%12===0?'C'+(pitch/12-1):''));
    for(let col=0;col<columns;col++){
      const at=col*step,note=t?.notes.find(n=>n.pitch===pitch&&n.startTick===at),cell=el('button','note-cell'+(col%2===0?' beat':'')+([1,3,6,8,10].includes(pitch%12)?' black':'')+(note?' filled':''));
      cell.setAttribute('role','gridcell');cell.setAttribute('aria-label',`Note ${pitch}, beat ${col/2+1}${note?', active':''}`);cell.tabIndex=note?0:-1;cell.disabled=!t||t.kind!=='notes'||t.locked||at>=total;
      cell.onclick=async()=>{remember();if(note)t.notes=t.notes.filter(n=>n.id!==note.id);else{t.notes.push({id:crypto.randomUUID(),pitch,startTick:at,durationTicks:480,velocity16:45000,expression:[]});await audio.ready();audio.voice(pitch,.6,t.instrument,audio.context.currentTime,.2,audio.buffers.get(t.assetId),t.sampleRoot??60);}changed();};
      grid.append(cell);
    }
  }
}
function renderTakes(){const box=$('#takes');box.replaceChildren();for(const t of project.tracks.filter(t=>t.kind==='audio')){const take=el('div','take',t.name);const player=document.createElement('audio');player.controls=true;player.src='/api/assets/'+t.assetId;take.append(player);box.append(take);}}
async function noteOn(pitch,velocity=.7,source='keys',velocity16=Math.round(velocity*65535)){
  const key=source+':'+pitch;if(held.has(key))return;
  const pending={release:()=>{},started:performance.now()};held.set(key,pending);
  try {await audio.ready();}catch(error){held.delete(key);throw error;}
  const t=track();const instrument=source==='pad'?'drums':t?.instrument??'keys';
  let buffer;if(t?.assetId&&t.kind==='notes')buffer=await audio.loadAsset(t.assetId);
  if(held.get(key)!==pending)return;
  const release=audio.voice(pitch,velocity,instrument,audio.context.currentTime,null,buffer,t?.sampleRoot??60);
  let note;
  if(audio.playing&&t?.armed&&!t.locked&&t.kind==='notes'){
    remember();note={id:crypto.randomUUID(),pitch,startTick:Math.round(audio.tick()/120)*120,durationTicks:120,velocity16:Math.max(1,velocity16),expression:[]};
    t.notes.push(note);changed();
  }
  held.set(key,{release,note,started:performance.now()});$$(`[data-pitch="${pitch}"]`).forEach(e=>e.classList.add('hit'));
}
function noteOff(pitch,source='keys'){const key=source+':'+pitch,h=held.get(key);if(!h)return;h.release();if(h.note){h.note.durationTicks=Math.max(120,Math.round((performance.now()-h.started)/1000*project.tempo/60*960/120)*120);changed();}held.delete(key);$$(`[data-pitch="${pitch}"]`).forEach(e=>e.classList.remove('hit'));}
function panic(){for(const key of [...held.keys()]){const separator=key.lastIndexOf(':');noteOff(Number(key.slice(separator+1)),key.slice(0,separator));}audio.stop();$$('.hit').forEach(e=>e.classList.remove('hit'));$('#play').textContent='▶ Play';}
function makePerformance(){
  const pads=$('#pads'),names=['KICK','SNARE','CLAP','HAT','LOW','TOM','RIM','OPEN','PERC','BELL','TICK','SHAKE','SNAP','RIDE','CRASH','TONE'],pitches=[36,38,39,42,41,43,37,46,60,56,75,70,40,51,49,64];
  names.forEach((name,i)=>{const button=el('button','pad',String(i+1).padStart(2,'0'));button.append(el('small','',name));button.dataset.pitch=pitches[i];button.setAttribute('aria-label',name+' pad');button.onpointerdown=e=>{button.setPointerCapture(e.pointerId);noteOn(pitches[i],.8,'pad').catch(e=>toast(e.message,true));};button.onpointerup=()=>{noteOff(pitches[i],'pad');};button.onpointercancel=button.onpointerup;pads.append(button);});
  const keyboard=$('#keyboard'),whites=[60,62,64,65,67,69,71,72,74,76,77,79,81,83],keys=['A','S','D','F','G','H','J','K','','','','','',''];
  for(const [i,pitch]of whites.entries()){const button=el('button','piano-key',keys[i]|| (pitch%12===0?'C'+(pitch/12-1):''));button.dataset.pitch=pitch;button.setAttribute('aria-label','Play note '+pitch);bindKey(button,pitch);keyboard.append(button);}
  for(const [i,pitch]of whites.entries())if([0,2,5,7,9].includes(pitch%12)){const button=el('button','piano-key sharp');button.style.left=((i+1)/whites.length*100-2.35)+'%';button.dataset.pitch=pitch+1;button.setAttribute('aria-label','Play note '+(pitch+1));bindKey(button,pitch+1);keyboard.append(button);}
  function bindKey(button,pitch){button.onpointerdown=e=>{button.setPointerCapture(e.pointerId);noteOn(pitch).catch(e=>toast(e.message,true));};button.onpointerup=()=>noteOff(pitch);button.onpointercancel=button.onpointerup;}
}
async function generate(pluginId,capability){
  await save();await post('/api/jobs',{projectId:project.id,projectRevision:project.revision,pluginId,capability,input:{prompt:$('#idea').value,seed:Number($('#seed').value),density:Number($('#density').value)/100,brightness:Number($('#density').value)/100}});await refreshJobs();
}
async function transcribeSelected(){
  const selected=track();if(!selected?.assetId)throw new Error('Select a recorded vocal or a sample track first');
  await save();await audio.ready();const buffer=await audio.loadAsset(selected.assetId);if(buffer.duration>120)throw new Error('Choose a take shorter than two minutes');
  const response=await fetch('/api/assets',{method:'POST',headers:{'Content-Type':'audio/wav'},body:bufferWAV(buffer)}),result=await response.json();if(!response.ok)throw new Error(result.error);
  await post('/api/jobs',{projectId:project.id,projectRevision:project.revision,pluginId:'spotify.basic-pitch',capability:'transcribe',input:{assetId:result.assetId,sourceAssetId:selected.assetId}});await refreshJobs();
}
function renderJobs(){
  $('#job-count').textContent=jobs.filter(j=>j.decision==='pending').length;const box=$('#jobs');box.replaceChildren();
  if(!jobs.length){box.append(el('p','muted','Suggestions appear here. Your session changes when you accept one.'));return;}
  for(const j of jobs.slice(0,12)){
    const card=el('div','candidate'),title=plugins.find(p=>p.id===j.pluginId)?.name??j.pluginId;card.append(el('h4','',title+' · '+(j.decision!=='pending'?j.decision:j.state)));
    card.append(el('p','',j.error??j.result?.summary??'Working in the background…'));
    if(j.result?.lyrics){const details=el('details');details.append(el('summary','','Read lyrics'),el('p','',j.result.lyrics));card.append(details);}
    const actions=el('div','candidate-buttons');
    function action(label,handler,primary=false){const b=el('button',primary?'primary':'quiet',label);b.onclick=()=>handler().catch(e=>toast(e.message,true));actions.append(b);}
    if(j.state==='succeeded'&&j.decision==='pending'){
      if(j.result?.tracks?.some(t=>t.notes.length))action('▶ Audition',async()=>{await audio.play({...project,tracks:j.result.tracks});$('#play').textContent='■ Stop';});
      if(j.result?.assetId)action('▶ Sample',async()=>{await audio.ready();const buffer=await audio.loadAsset(j.result.assetId);audio.voice(60,.8,'sample',audio.context.currentTime,2,buffer);});
      action('Accept',async()=>{await save();const accepted=await post('/api/jobs/'+j.id+'/accept',{expectedRevision:project.revision});remember();project=accepted.project;selectedId=project.tracks.at(-1)?.id;renderStudio();await refreshJobs();toast('Added to your session');},true);
      action('Discard',async()=>{await post('/api/jobs/'+j.id+'/reject',{});await refreshJobs();});
    }else if(['queued','running'].includes(j.state))action('Cancel',async()=>{await post('/api/jobs/'+j.id+'/cancel',{});await refreshJobs();});
    card.append(actions);box.append(card);
  }
}
async function refreshJobs(){const next=await api('/api/jobs?projectId='+project.id);if(JSON.stringify(next)!==JSON.stringify(jobs)){jobs=next;renderJobs();}}
function renderModels(){
  const search=$('#model-search').value.toLowerCase(),category=$('#model-category').value,grid=$('#model-grid');grid.replaceChildren();
  const models=catalog.models.filter(m=>(!category||m.category===category)&&JSON.stringify(m).toLowerCase().includes(search));$('#model-count').textContent=models.length+' candidates';
  for(const m of models){const card=el('article','panel model-card');card.append(el('span','eyebrow',m.category.toUpperCase()),el('h2','',m.name),el('p','',m.summary),el('p','controls',m.controls),el('small','',m.caveat));
    const source=el('a','','Read the primary source ↗');source.href=m.source;source.target='_blank';source.rel='noreferrer';card.append(source);
    const select=el('select');select.setAttribute('aria-label','Selection for '+m.name);for(const [value,label]of [['explore','Explore'],['keep','Keep in the rig'],['skip','Skip for now']]){const option=el('option','',label);option.value=value;select.append(option);}select.value=selections[m.id]??m.recommendation;
    select.onchange=async()=>{selections[m.id]=select.value;try{await api('/api/selections',{method:'PUT',body:JSON.stringify(selections)});toast('Model selection saved');}catch(e){toast(e.message,true);}};card.append(select);grid.append(card);
  }
}
function download(value,name,type='application/json'){const blob=new Blob([typeof value==='string'||value instanceof Blob?value:JSON.stringify(value,null,2)],{type});const link=document.createElement('a');link.href=URL.createObjectURL(blob);link.download=name;link.click();setTimeout(()=>URL.revokeObjectURL(link.href),1000);}
async function recordVoice(){
  if(recording){recording.recorder.stop();return;}
  if(!navigator.mediaDevices?.getUserMedia||!window.MediaRecorder)throw new Error('Microphone capture is unavailable here. Open the local studio in Chrome.');
  const stream=await navigator.mediaDevices.getUserMedia({audio:{echoCancellation:false,noiseSuppression:false,autoGainControl:false}}),chunks=[];
  const supported=['audio/webm;codecs=opus','audio/mp4','audio/ogg;codecs=opus'].find(t=>MediaRecorder.isTypeSupported(t));
  const recorder=new MediaRecorder(stream,supported?{mimeType:supported}:{});const button=$('#record');button.classList.add('recording');button.textContent='■ Finish take';
  const projectId=project.id;
  const timer=setTimeout(()=>recorder.state==='recording'&&recorder.stop(),120000);
  recording={recorder,stream};recorder.ondataavailable=e=>{if(e.data.size)chunks.push(e.data);};
  recorder.onstop=async()=>{
    clearTimeout(timer);stream.getTracks().forEach(t=>t.stop());recording=null;button.classList.remove('recording');button.textContent='● Record voice';
    try{const blob=new Blob(chunks,{type:recorder.mimeType});const response=await fetch('/api/assets',{method:'POST',headers:{'Content-Type':recorder.mimeType.split(';')[0]},body:blob});const result=await response.json();if(!response.ok)throw new Error(result.error);if(project.id!==projectId)throw new Error('Take saved as an asset, but the session changed during recording');
      remember();project.tracks.push({id:crypto.randomUUID(),name:'Your voice · take '+(project.tracks.filter(t=>t.kind==='audio').length+1),kind:'audio',instrument:'voice',gain:.7,muted:false,locked:false,notes:[],assetId:result.assetId});changed();toast('Original vocal take saved locally');
    }catch(e){toast(e.message,true);}
  };recorder.start(1000);
}
function midiNote(status,pitch,velocity,velocity16){const kind=status&0xf0;if(kind===0x90&&velocity>0)noteOn(pitch,velocity,'midi',velocity16).catch(e=>toast(e.message,true));else if(kind===0x80||kind===0x90)noteOff(pitch,'midi');}
window.omniuNative=message=>{
  if(message.type==='status')$('#midi-status').textContent=message.message;
  if(message.type==='ump')for(let i=0;i<message.words.length;){
    const w=message.words[i]>>>0,mt=w>>>28;
    if(mt===2){midiNote((w>>>16)&255,(w>>>8)&127,(w&127)/127,Math.round((w&127)/127*65535));i++;}
    else if(mt===4&&i+1<message.words.length){const vel=(message.words[i+1]>>>16)&65535;midiNote((w>>>16)&255,(w>>>8)&127,vel/65535,vel);i+=2;}
    else i+=[1,1,1,2,2,4,1,1,2,2,2,3,3,4,4,4][mt];
  }
};
async function connectMIDI(){
  await audio.ready();
  if(window.webkit?.messageHandlers?.omniu){window.webkit.messageHandlers.omniu.postMessage({action:'connectMIDI'});return;}
  if(!navigator.requestMIDIAccess)throw new Error('Use the Mac app or Chrome for MIDI input. The on-screen keys are ready here.');
  midiAccess=await navigator.requestMIDIAccess({sysex:false});
  const bind=()=>{for(const input of midiAccess.inputs.values())input.onmidimessage=e=>midiNote(e.data[0],e.data[1],e.data[2]/127,Math.round(e.data[2]/127*65535));$('#midi-status').textContent=`${midiAccess.inputs.size} MIDI 1 input(s) · Web MIDI`;};bind();midiAccess.onstatechange=bind;
}
async function boot(){
  [catalog,plugins,selections]=await Promise.all([api('/api/catalog'),api('/api/plugins'),api('/api/selections')]);
  const projects=await api('/api/projects');projects.sort((a,b)=>(b.updatedAt??b.createdAt).localeCompare(a.updatedAt??a.createdAt));project=await api('/api/projects/'+projects[0].id);selectedId=project.tracks[0]?.id;
  for(const category of [...new Set(catalog.models.map(m=>m.category))]){const option=el('option','',category);option.value=category;$('#model-category').append(option);}
  makePerformance();renderStudio();renderModels();await refreshJobs();
  $$('.rail-button').forEach(b=>b.onclick=()=>{$$('.rail-button').forEach(x=>x.classList.toggle('active',x===b));$$('.view').forEach(v=>v.classList.toggle('hidden',v.id!==b.dataset.view+'-view'));});
  const guarded=(selector,fn)=>{$(selector).onclick=()=>Promise.resolve().then(fn).catch(e=>toast(e.message,true));};
  guarded('#save',save);guarded('#play',async()=>{if(audio.playing)panic();else{await audio.play(project);$('#play').textContent='■ Stop';}});guarded('#panic',async()=>panic());
  guarded('#sketch',()=>generate('local.sketch','compose'));guarded('#instrument',()=>generate('local.instrument','instrument'));guarded('#lyrics',()=>generate($('#use-ollama').checked?'ollama.lyrics':'local.lyric-cards','lyrics'));guarded('#transcribe',transcribeSelected);guarded('#record',recordVoice);guarded('#midi-connect',connectMIDI);
  guarded('#undo',async()=>{if(!undoStack.length)return;const revision=project.revision;project=undoStack.pop();project.revision=revision;selectedId=track()?.id;changed();});
  guarded('#clear-track',async()=>{if(track()?.locked)return;remember();track().notes=[];changed();});
  guarded('#add-track',async()=>{remember();const t={id:crypto.randomUUID(),name:'Instrument '+(project.tracks.length+1),kind:'notes',instrument:'keys',gain:.65,muted:false,locked:false,notes:[]};project.tracks.push(t);selectedId=t.id;changed();});
  guarded('#new-project',async()=>{if(recording)throw new Error('Finish your vocal take before starting a new session');await save();panic();project=await post('/api/projects',{name:'Untitled session'});selectedId=project.tracks[0].id;undoStack=[];renderStudio();await refreshJobs();});
  $('#project-name').onchange=()=>{remember();project.name=$('#project-name').value.trim()||'Untitled session';changed();};
  $('#tempo').onchange=()=>{const value=Number($('#tempo').value);if(value<30||value>300){toast('Choose a tempo from 30 to 300 BPM',true);$('#tempo').value=project.tempo;return;}remember();panic();project.tempo=value;changed();};
  $('#lyric-editor').oninput=()=>{if(project.lyricsLocked)return;project.lyrics=$('#lyric-editor').value;changed(false);};$('#lock-lyrics').onchange=()=>{remember();project.lyricsLocked=$('#lock-lyrics').checked;changed();};
  guarded('#export-project',async()=>{await save();download(project,'omniusong-project.json');});
  guarded('#export-midi',async()=>{await save();const response=await fetch('/api/projects/'+project.id+'/midi');if(!response.ok)throw new Error('MIDI export failed');download(await response.blob(),'omniusong.mid','audio/midi');const warning=decodeURIComponent(response.headers.get('X-OmniuSong-Warnings')??'');toast(warning||'MIDI 1 exported. Your full expression data stays in the project.');});
  guarded('#export-ump',async()=>{await save();download(await api('/api/projects/'+project.id+'/ump'),'omniusong.ump.json');toast('MIDI 2 UMP packet trace exported. Standard Clip File export is planned.');});
  $('#model-search').oninput=renderModels;$('#model-category').onchange=renderModels;
  const keys={a:60,s:62,d:64,f:65,g:67,h:69,j:71,k:72};
  window.addEventListener('keydown',e=>{if(e.repeat||/INPUT|TEXTAREA|SELECT/.test(e.target.tagName)||e.metaKey||e.ctrlKey||e.altKey)return;if(keys[e.key]){e.preventDefault();noteOn(keys[e.key]).catch(err=>toast(err.message,true));}if(e.code==='Space'){e.preventDefault();$('#play').click();}});
  window.addEventListener('keyup',e=>{if(keys[e.key])noteOff(keys[e.key]);});window.addEventListener('blur',()=>{for(const key of [...held.keys()]){const [source,pitch]=key.split(':');noteOff(Number(pitch),source);}});
  window.addEventListener('beforeunload',e=>{if(dirty||recording){e.preventDefault();e.returnValue='';}});
  setInterval(()=>{if(!document.hidden&&!dirty&&!savePromise)refreshJobs().catch(()=>{});},1500);
}
boot().catch(e=>toast('Could not open studio: '+e.message,true));
