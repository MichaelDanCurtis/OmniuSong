import { StudioAudio, bufferWAV } from './audio.js';
import { PerformanceInput, decodeMIDI1, decodeUMP, appendCapture } from './input.js';
import { TakeCapture, audioType, AUDIO_LIMIT_BYTES, recordingTiming, takePlacement, takeAssetId, sameTakeSource } from './recording.js';
import { rollWindow, candidateAudition } from './note-view.js';
import { editNote, restoreNote, deleteNote } from './note-edit.js';
import { clipWindow, editClip, restoreClip, analysisWindow } from './clip-edit.js';
import { transportSettings } from './transport.js';
const $=s=>document.querySelector(s),$$=s=>[...document.querySelectorAll(s)],audio=new StudioAudio();
let project,selectedId,catalog,plugins,selections={},jobs=[],dirty=false,editSequence=0,savePromise=null,saveTimer,undoStack=[],midiAccess,sessionBusy=false,rollPage=0,rollTop=null,selectedNoteId=null,noteDraft=null,clipDraft=null;
const midiConnected=new Map(),nativeSources=new Set();
const colors=['#bfdb9f','#e5b677','#97bec1','#cd9dad','#9eb7d5','#d6cb90'];
const localAddress=sourceId=>({sourceId,sourceName:sourceId==='computer'?'Computer keyboard':'On-screen keys',group:0,channel:0,protocol:'local'});
const input=new PerformanceInput({
  async startVoice(voice){
    await audio.ready();let buffer;const t=voice.track;
    if(t?.assetId&&t.kind==='notes')buffer=await audio.loadAsset(t.assetId);
    if(voice.ended||t?.muted)return;
    return audio.voice(voice.pitch,voice.velocity16/65535,voice.instrument,audio.context.currentTime,null,buffer,t?.sampleRoot??60,t?.gain??.65,t?.id);
  },
  onStart(voice){
    const t=voice.track;
    if(!audio.recordingReady()||!t?.armed||t.locked||t.kind!=='notes')return;
    remember();
    const note={id:voice.id,pitch:voice.pitch,startTick:Math.floor(audio.tick()),durationTicks:1,velocity16:voice.velocity16,expression:[],
      input:{sourceId:voice.sourceId,sourceName:voice.sourceName,protocol:voice.protocol,group:voice.group,channel:voice.channel,receivedTimeMs:voice.startedAt}};
    t.notes.push(note);changed();return {note,projectId:project.id,tempo:project.tempo};
  },
  onKeyRelease(voice){
    if(!voice.record||voice.record.projectId!==project.id)return;
    voice.record.note.input.keyDurationTicks=durationTicks(voice.record,voice.keyReleasedAt-voice.startedAt);
    voice.record.note.input.releaseVelocity16=voice.releaseVelocity16;changed(false);
  },
  onEnd(voice,reason){
    if(!voice.record||voice.record.projectId!==project.id)return;
    voice.record.note.durationTicks=durationTicks(voice.record,voice.endedAt-voice.startedAt);
    voice.record.note.input.endReason=reason;changed();
  },
  onChange(state){
    const pitches=new Set([...state.voices.values()].map(v=>v.pitch));
    $$('[data-pitch]').forEach(e=>e.classList.toggle('hit',pitches.has(Number(e.dataset.pitch))));
    const pedal=$('#sustain');if(pedal)pedal.setAttribute('aria-pressed',String(state.pedals.has(JSON.stringify(['computer',0,0]))));
    const status=$('#input-status');if(status)status.textContent=`${state.voices.size} active note${state.voices.size===1?'':'s'} · Sustain ${state.pedals.size?'on':'off'}`;
  },
  onError:error=>toast(error.message,true)
});
function durationTicks(record,milliseconds){return Math.max(1,Math.min(1e8,Math.round(milliseconds/1000*record.tempo/60*960)));}
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
  try{const saved=await savePromise;project.revision=saved.revision;project.updatedAt=saved.updatedAt;if(sequence===editSequence){dirty=false;$('#save-status').textContent=noteFieldPending()?'Note edit pending':clipFieldPending()?'Clip edit pending':'Saved on this Mac';}else $('#save-status').textContent='Unsaved changes';$('#session-meta').textContent=`${project.bars} bars · ${project.tracks.reduce((s,t)=>s+t.notes.length,0)} notes · Revision ${project.revision}`;const option=[...$('#project-picker').options].find(o=>o.value===project.id);if(option)option.textContent=project.name;}
  catch(error){$('#save-status').textContent='Save failed · changes retained';throw error;}
  finally{savePromise=null;}
}
async function flushSaves(){while(dirty||savePromise)await save();clearTimeout(saveTimer);}
async function refreshLibrary(){
  const sessions=await api('/api/projects');sessions.sort((a,b)=>(b.updatedAt??b.createdAt).localeCompare(a.updatedAt??a.createdAt));
  const picker=$('#project-picker');picker.replaceChildren();for(const session of sessions){const option=el('option','',session.name);option.value=session.id;picker.append(option);}if(project)picker.value=project.id;return sessions;
}
async function useProject(next){
  project=next;selectedId=next.tracks[0]?.id;selectedNoteId=null;noteDraft=null;clipDraft=null;rollPage=0;rollTop=null;$('#audio-start-bar').value=1;$('#aria-start').value=1;$('#aria-bars').value=Math.min(2,next.bars);$('#magenta-start').value=1;undoStack=[];jobs=[];dirty=false;editSequence=0;clearTimeout(saveTimer);
  try{localStorage.setItem('omniusong.activeProject',next.id);}catch{}
  renderStudio();renderJobs();updateCaptureControls();await refreshLibrary();await refreshJobs();$('#save-status').textContent='Saved on this Mac';
}
async function sessionTransition(operation){
  if(sessionBusy)throw new Error('A session is already opening');
  if(capture.busy)throw new Error('Finish saving the original audio before changing sessions');
  if(!commitPendingEdits())throw new Error('Edit rejected; review the selected track before changing sessions');
  sessionBusy=true;$('main').inert=true;for(const id of ['#project-picker','#new-project','#import-project','#export-project','#record','#play','#save','#tempo'])$(id).disabled=true;
  $('#library-status').textContent='Saving and opening…';
  try{panic();await flushSaves();await operation();}
  finally{sessionBusy=false;$('main').inert=false;for(const id of ['#project-picker','#new-project','#import-project','#export-project','#record','#play','#save','#tempo'])$(id).disabled=false;$('#project-picker').value=project.id;$('#library-status').textContent='Backups include your audio.';updateCaptureControls();}
}
async function restoreBackup(file){
  if(!file)return;
  if(file.size>48*1024*1024)throw new Error('Choose a backup no larger than 48 MiB');
  await sessionTransition(async()=>{
    $('#library-status').textContent='Checking backup and restoring audio…';
    const response=await fetch('/api/archives',{method:'POST',headers:{'Content-Type':'application/vnd.omniusong.project+gzip'},body:file});
    const restored=await response.json();if(!response.ok)throw new Error(restored.error);
    await useProject(restored.project);toast(`Restored a new session with ${restored.assetCount} audio asset(s)`);
  });
}
function renderStudio(){
  $('#project-name').value=project.name;$('#tempo').value=project.tempo;$('#session-meta').textContent=`${project.bars} bars · ${project.tracks.reduce((s,t)=>s+t.notes.length,0)} notes · Revision ${project.revision}`;
  $('#track-count').textContent=`${project.tracks.length} TRACKS`;$('#selected-track').textContent=track()?.name??'Add a track';
  $('#clear-track').disabled=Boolean(track()?.locked);$('#undo').disabled=!undoStack.length;
  const arrangement=$('#arrangement');arrangement.replaceChildren();
  const ruler=el('div','bar-ruler');ruler.append(el('span','','TRACKS'));const numbers=el('div','bar-numbers');for(let b=1;b<=project.bars;b++)numbers.append(el('span','',String(b).padStart(2,'0')));ruler.append(numbers);arrangement.append(ruler);
  project.tracks.forEach((t,index)=>{
    const row=el('div','track-row'+(t.id===selectedId?' selected':''));row.style.setProperty('--track-color',colors[index%colors.length]);const label=el('div','track-label');label.append(el('span','track-dot'));
    const select=el('button','track-select',t.name);select.title=t.name;select.onclick=()=>selectTrack(t.id);label.append(select);
    for(const [field,text]of [['muted','M'],['locked','L']]){const button=el('button','track-switch'+(t[field]?' enabled':''),text);button.title=(field==='muted'?'Mute ':'Lock ')+t.name;button.setAttribute('aria-label',button.title);button.setAttribute('aria-pressed',String(t[field]));button.onclick=()=>{if(!commitPendingEdits())return;const current=project.tracks.find(x=>x.id===t.id);remember();current[field]=!current[field];if(field==='muted')audio.setTrackMuted(current.id,current.muted);changed();};label.append(button);}
    if(t.kind==='notes'){const arm=el('button','track-switch'+(t.armed?' enabled':''),'R');arm.title='Arm note recording for '+t.name;arm.setAttribute('aria-label',arm.title);arm.setAttribute('aria-pressed',String(Boolean(t.armed)));arm.onclick=()=>{if(!commitPendingEdits())return;const current=project.tracks.find(x=>x.id===t.id);remember();current.armed=!current.armed;changed();};label.append(arm);}
    const lane=el('div','track-lane'+(t.notes.length||t.assetId?' has-notes':''));
    if(t.notes.length)for(const n of t.notes){const note=el('span','mini-note');note.style.left=(n.startTick/(project.bars*3840)*100)+'%';note.style.width=Math.max(.4,n.durationTicks/(project.bars*3840)*100)+'%';note.style.top=(5+(n.pitch%12)*2)+'px';lane.append(note);}
    else {
      const label=t.assetId?(t.kind==='audio'?(t.audio?.origin==='generated'?'◉ Generated AI audio':t.audio?.origin==='import'?'◉ Imported original audio':'◉ Original vocal take'):'◈ Playable sample'):'Your next idea goes here';
      const clip=el('span','empty-lane',label);
      if(t.kind==='audio'&&t.audio){const view=clipWindow(t.audio);clip.classList.add('audio-clip');clip.style.left=(view.startTick/(project.bars*3840)*100)+'%';clip.style.width=(view.durationSeconds*project.tempo/60*960/(project.bars*3840)*100)+'%';clip.title=label+` · bar ${(view.startTick/3840+1).toFixed(2)} · source ${view.trimStartSeconds}–${view.trimEndSeconds} seconds`;}
      lane.append(clip);
    }
    row.append(label,lane);arrangement.append(row);
  });
  const isAudio=track()?.kind==='audio';$('#editor-mode').textContent=isAudio?'AUDIO CLIP':'NOTE EDITOR';
  for(const selector of ['.roll-navigation','#piano-roll','#note-inspector','#clear-track','.editor-actions .tiny-tag'])$(selector).classList.toggle('hidden',isAudio);
  $('#clip-inspector').classList.toggle('hidden',!isAudio);$('#editor-hint').textContent=isAudio?'Move or trim the arrangement clip. The complete source stays in its player and download.':'Click an empty cell to write; click a note to edit. Arm R and use Play to record keys after the count-in.';
  if(!isAudio)renderRoll();renderNoteInspector();renderClipInspector();renderTakes();renderTransportControls();
  $('#aria-source').textContent='Source: '+(track()?.name??'Select a pitched track');$('#aria-start').max=project.bars;
  $('#magenta-source').textContent='Selected guide: '+(track()?.name??'Select a pitched track');$('#magenta-start').max=project.bars;
  if(document.activeElement!==$('#lyric-editor'))$('#lyric-editor').value=project.lyrics;
  $('#lock-lyrics').checked=Boolean(project.lyricsLocked);$('#lyric-editor').disabled=Boolean(project.lyricsLocked);
}
function selectTrack(id){
  if(!commitPendingEdits())return;
  selectedId=id;selectedNoteId=null;noteDraft=null;clipDraft=null;rollTop=null;const notes=track()?.notes??[];
  rollPage=notes.length?Math.floor(Math.min(...notes.map(n=>n.startTick))/15360):0;renderStudio();
}
function renderRoll(){
  const grid=$('#piano-roll');grid.replaceChildren();const t=track();const total=project.bars*3840,step=480,columns=32;
  grid.style.gridTemplateColumns=`28px repeat(${columns}, minmax(4px,1fr))`;
  if(rollTop===null)rollTop=t?.notes.length?Math.min(127,Math.max(24,Math.ceil(Math.max(...t.notes.map(n=>n.pitch))/12)*12)):(t?.instrument==='drums'?60:t?.instrument==='bass'?48:72);
  const view=rollWindow(project.bars,rollPage,rollTop),top=view.topPitch;rollPage=view.page;
  $('#roll-range').textContent=`Bars ${view.startBar}–${view.endBar} of ${project.bars}`;$('#roll-prev').disabled=rollPage===0;$('#roll-next').disabled=rollPage===view.pages-1;
  const pages=$('#roll-page');pages.replaceChildren();for(let page=0;page<view.pages;page++){const option=el('option','',`Bars ${page*4+1}–${Math.min(project.bars,(page+1)*4)}`);option.value=page;pages.append(option);}pages.value=rollPage;
  $('#roll-pitch').value=top;
  for(let pitch=top;pitch>=top-24;pitch--){
    grid.append(el('span','pitch-name',pitch%12===0?'C'+(pitch/12-1):''));
    for(let col=0;col<columns;col++){
      const at=view.startTick+col*step,notes=t?.notes.filter(n=>n.pitch===pitch&&n.startTick>=at&&n.startTick<at+step)??[],note=notes[0],chosen=notes.some(n=>n.id===selectedNoteId),cell=el('button','note-cell'+(col%2===0?' beat':'')+([1,3,6,8,10].includes(pitch%12)?' black':'')+(note?' filled':'')+(chosen?' selected-note':''));
      if(note)cell.title=`${notes.length>1?notes.length+' notes in this cell. Click again to cycle. ':''}Start tick ${note.startTick} · Length ${note.durationTicks} ticks. Select to edit.`;
      cell.setAttribute('role','gridcell');cell.setAttribute('aria-label',`Note ${pitch}, beat ${at/960+1}${note?', active':''}`);cell.setAttribute('aria-selected',String(chosen));cell.tabIndex=note?0:-1;cell.disabled=!t||t.kind!=='notes'||(t.locked&&!note)||at>=total;
      cell.onclick=async()=>{
        try{
          if(note){selectNote(notes[(notes.findIndex(n=>n.id===selectedNoteId)+1)%notes.length].id);return;}
          if(!commitPendingEdits())return;
          const current=track();if(!current||current.locked||current.kind!=='notes')return;
          panic();remember();const added={id:crypto.randomUUID(),pitch,startTick:at,durationTicks:480,velocity16:45000,expression:[]};current.notes.push(added);selectedNoteId=added.id;noteDraft=null;changed();
          await audio.ready();audio.voice(pitch,.6,current.instrument,audio.context.currentTime,.2,audio.buffers.get(current.assetId),current.sampleRoot??60);
        }catch(error){toast(error.message,true);}
      };
      grid.append(cell);
    }
  }
}
const noteName=pitch=>['C','C♯','D','D♯','E','F','F♯','G','G♯','A','A♯','B'][pitch%12]+(Math.floor(pitch/12)-1);
function selectedNote(){return track()?.notes.find(n=>n.id===selectedNoteId);}
function locateSelectedNote(){
  const note=selectedNote();
  if(note){rollPage=Math.floor(note.startTick/15360);if(rollTop===null||note.pitch>rollTop||note.pitch<rollTop-24)rollTop=Math.min(127,Math.max(24,Math.ceil(note.pitch/12)*12));}
}
function selectNote(id){
  if(!commitPendingEdits())return;
  selectedNoteId=id;noteDraft=null;locateSelectedNote();
  renderRoll();renderNoteInspector();
}
function noteFieldPending(){
  const note=selectedNote();return Boolean(note&&noteDraft?.noteId===note.id&&noteDraft.trackId===track()?.id&&Object.entries(noteDraft.patch).some(([key,value])=>value===''||Number(value)!==note[key]));
}
function commitPendingNote(){
  if(!noteFieldPending()){if(noteDraft){noteDraft=null;if(!dirty&&!savePromise)$('#save-status').textContent='Saved on this Mac';}return true;}
  return commitNotePatch(Object.fromEntries(Object.entries(noteDraft.patch).map(([key,value])=>[key,value===''?NaN:Number(value)])));
}
function renderNoteInspector(){
  const t=track(),note=selectedNote(),picker=$('#note-picker');picker.replaceChildren(el('option','','Choose a note…'));
  picker.options[0].value='';const sorted=[...(t?.notes??[])].sort((a,b)=>a.startTick-b.startTick||a.pitch-b.pitch||a.id.localeCompare(b.id));
  for(const [index,n]of sorted.entries()){const option=el('option','',`${index+1}. ${noteName(n.pitch)} · tick ${n.startTick} · ${n.durationTicks} long`);option.value=n.id;picker.append(option);}
  picker.value=note?.id??'';picker.disabled=!sorted.length;
  for(const field of $$('[data-note-field]')){
    field.disabled=!note||t.locked;
    const draft=noteDraft&&noteDraft.noteId===note?.id&&noteDraft.trackId===t?.id?noteDraft.patch:{};
    field.value=Object.hasOwn(draft,field.dataset.noteField)?draft[field.dataset.noteField]:note?.[field.dataset.noteField]??'';
  }
  $('#note-save').disabled=!note||t.locked;$('#note-delete').disabled=!note||t.locked;$('#note-restore').disabled=!note||t.locked||!note.editSource;
  $('#note-position').textContent=note?`${noteName(note.pitch)} · beat ${(note.startTick/960+1).toFixed(4)} · ${(note.durationTicks/960).toFixed(4)} beats long · ${(note.velocity16/65535*100).toFixed(1)}% velocity`:t?.kind==='audio'?'Select a note track to edit its notes.':'Click a note, or choose one above. Repeated notes in one cell are all listed.';
  $('#note-source').textContent=note?`${note.input?'Input: '+note.input.sourceName+'. ':''}${(note.expression??[]).reduce((sum,lane)=>sum+lane.points.length,0)} expression points. ${note.editSource?'Original values kept; Restore original reverses these edits.':'Input and model metadata stay intact.'}`:'';
  $('#note-edit-status').textContent=t?.locked?'Track locked · notes can be inspected.':noteFieldPending()?'Save note or press Enter. Leaving this note also saves.':'960 ticks = 1 beat. Save note or press Enter after editing. Undo reverses an edit.';
}
function commitNotePatch(patch){
  const id=selectedNoteId,trackId=track()?.id;
  try{
    panic();const next=editNote(project,trackId,id,patch);noteDraft=null;if(next!==project){remember();project=next;locateSelectedNote();changed();}
    renderNoteInspector();if(!dirty&&!savePromise)$('#save-status').textContent='Saved on this Mac';return true;
  }catch(error){noteDraft=null;renderNoteInspector();$('#note-edit-status').textContent=error.message;if(!dirty&&!savePromise)$('#save-status').textContent='Saved on this Mac';toast(error.message,true);return false;}
}
function transformSelectedNote(operation){
  panic();const next=operation(project,track()?.id,selectedNoteId);if(next!==project){remember();project=next;locateSelectedNote();changed();}
}
function clipFieldPending(){
  const t=track();return Boolean(t?.audio&&clipDraft?.trackId===t.id&&Object.entries(clipDraft.patch).some(([key,value])=>value===''||Number(value)!==clipWindow(t.audio)[key]));
}
function commitPendingClip(){
  if(!clipFieldPending()){if(clipDraft){clipDraft=null;if(!dirty&&!savePromise)$('#save-status').textContent='Saved on this Mac';}return true;}
  try{
    panic();const patch=Object.fromEntries(Object.entries(clipDraft.patch).map(([key,value])=>[key,value===''?NaN:Number(value)])),next=editClip(project,track()?.id,patch);clipDraft=null;
    if(next!==project){remember();project=next;changed();}renderClipInspector();return true;
  }catch(error){clipDraft=null;renderClipInspector();$('#clip-edit-status').textContent=error.message;if(!dirty&&!savePromise)$('#save-status').textContent='Saved on this Mac';toast(error.message,true);return false;}
}
function commitPendingEdits(){return commitPendingNote()&&commitPendingClip();}
function renderClipInspector(){
  const t=track(),view=t?.kind==='audio'&&t.audio?clipWindow(t.audio):null;
  for(const field of $$('[data-clip-field]')){const key=field.dataset.clipField,draft=clipDraft?.trackId===t?.id?clipDraft?.patch:{};field.value=draft&&Object.hasOwn(draft,key)?draft[key]:view?.[key]??'';field.disabled=!view||t.locked;if(key!=='startTick')field.max=t?.audio?.durationSeconds??86400;}
  $('#clip-save').disabled=!view||t.locked;$('#clip-restore').disabled=!view||t.locked||!t.audio.editSource;$('#clip-preview').disabled=!view;
  $('#clip-position').textContent=view?`Bar ${(view.startTick/3840+1).toFixed(4)} · beat ${(view.startTick/960+1).toFixed(4)} · ${view.durationSeconds.toFixed(4)} seconds in the arrangement`:'';
  $('#clip-source').textContent=view?`Full ${t.audio.example&&t.audio.recordingTiming?'simulated recording':t.audio.origin==='generated'?'AI-generated':t.audio.origin==='import'?'imported':'recorded'} source: ${t.audio.durationSeconds.toFixed(4)} seconds. ${t.audio.editSource?'Original placement and trim kept. ':''}The source file and its metadata stay intact. ${t.audio.recordingTiming?(t.audio.recordingTiming.status==='estimated'?'Timing estimated from the recorder start command; microphone and encoder latency are unmeasured. Adjust Start / Source in after listening.':t.audio.recordingTiming.status==='ended-before-song'?'Take ended before the song boundary. Full source placed at bar 1 for review.':'Automatic timing was outside 128 bars. Full source placed at bar 1 for review.'):''}`:'';
  $('#clip-edit-status').textContent=t?.locked?'Track locked · clip can be inspected and auditioned.':clipFieldPending()?'Save clip or press Enter. Changing tracks also saves.':'960 ticks = 1 beat. Source in/out use seconds from the full recording.';
}
function renderTakes(){const box=$('#takes');box.replaceChildren();for(const t of project.tracks.filter(t=>t.kind==='audio')){const take=el('div','take',t.name);const player=document.createElement('audio');player.controls=true;player.src='/api/assets/'+t.assetId;player.setAttribute('aria-label',t.name);const original=el('a','original-audio',t.audio?.origin==='generated'?'Download generated WAV':'Download original');original.href=player.src;original.download=t.audio?.originalName??('OmniuSong-original.'+t.assetId.split('.').at(-1));take.append(player,original);box.append(take);}}
function noteOn(pitch,velocity=.7,sourceId='screen',extra={}){
  if(sessionBusy||!project)return;
  const t=track();
  return input.noteOn({...localAddress(sourceId),pitch,velocity16:Math.round(velocity*65535),...extra,track:t,instrument:sourceId==='pads'?'drums':t?.instrument??'keys'});
}
function noteOff(pitch,sourceId='screen',voiceId){input.noteOff({...localAddress(sourceId),pitch,voiceId});}
function panic(){input.panic();if(capture.take?.context.withSong&&['requesting','preparing','recording'].includes(capture.phase))capture.stop();audio.stop();$('#play').textContent='▶ Play';}
function renderTransportControls(){
  const settings=transportSettings(project.transport);$('#metronome').setAttribute('aria-pressed',String(settings.metronome));$('#count-in').value=settings.countInBars;$('#click-level').value=Math.round(settings.clickLevel*100);$('#count-in').disabled=audio.playing||capture.busy;
}
function updateTransport(_tick,state=audio.position()){
  const label=state.phase==='loading'?'Loading audio…':state.phase==='starting'?'Starting…':state.phase==='count-in'?`Count in · ${state.countInBeat} / ${state.countInBeats}`:state.phase==='playing'?`${state.mode==='audition'?'Audition':'Playing'} · bar ${state.bar} · beat ${state.beat}`:'Stopped';
  if($('#transport-position').textContent!==label)$('#transport-position').textContent=label;
  $('#transport-position').classList.toggle('counting',state.phase==='count-in');$('#play').textContent=audio.playing?'■ Stop':'▶ Play';$('#count-in').disabled=audio.playing||capture.busy;
}
function changeTransport(patch){
  if(patch.countInBars!==undefined&&(audio.playing||capture.busy))throw new Error('Finish playback and recording before changing the count-in');
  const settings=transportSettings({...transportSettings(project.transport),...patch});remember();project.transport=settings;audio.setMetronome(settings.metronome,settings.clickLevel);changed();
}
async function startPlayback(song=project,mode='session',{captureStart=false}={}){
  if(capture.busy&&!captureStart)throw new Error('Finish the current voice take before starting playback or auditioning');
  input.panic('transport-restart');
  try{await audio.play(song,updateTransport,{mode});}finally{updateTransport();}
}
function setLocalSustain(down){for(const sourceId of ['computer','screen'])input.sustain(localAddress(sourceId),down);}
function makePerformance(){
  const pads=$('#pads'),names=['KICK','SNARE','CLAP','HAT','LOW','TOM','RIM','OPEN','PERC','BELL','TICK','SHAKE','SNAP','RIDE','CRASH','TONE'],pitches=[36,38,39,42,41,43,37,46,60,56,75,70,40,51,49,64];
  names.forEach((name,i)=>{const button=el('button','pad',String(i+1).padStart(2,'0'));button.append(el('small','',name));button.dataset.pitch=pitches[i];button.setAttribute('aria-label',name+' pad');bindKey(button,pitches[i],'pads');pads.append(button);});
  const keyboard=$('#keyboard'),whites=[60,62,64,65,67,69,71,72,74,76,77,79,81,83],keys=['A','S','D','F','G','H','J','K','','','','','',''];
  for(const [i,pitch]of whites.entries()){const button=el('button','piano-key',keys[i]|| (pitch%12===0?'C'+(pitch/12-1):''));button.dataset.pitch=pitch;button.setAttribute('aria-label','Play note '+pitch);bindKey(button,pitch);keyboard.append(button);}
  for(const [i,pitch]of whites.entries())if([0,2,5,7,9].includes(pitch%12)){const button=el('button','piano-key sharp');button.style.left=((i+1)/whites.length*100-2.35)+'%';button.dataset.pitch=pitch+1;button.setAttribute('aria-label','Play note '+(pitch+1));bindKey(button,pitch+1);keyboard.append(button);}
  function bindKey(button,pitch,sourceId='screen'){const pointers=new Map();button.onpointerdown=e=>{if(e.button!==0)return;e.preventDefault();button.setPointerCapture(e.pointerId);pointers.set(e.pointerId,noteOn(pitch,sourceId==='pads'?.8:.7,sourceId));};button.onpointerup=e=>{const voiceId=pointers.get(e.pointerId);if(voiceId)noteOff(pitch,sourceId,voiceId);pointers.delete(e.pointerId);};button.onpointercancel=button.onpointerup;button.onlostpointercapture=button.onpointerup;}
}
async function generate(pluginId,capability){
  await sessionTransition(async()=>{await post('/api/jobs',{projectId:project.id,projectRevision:project.revision,pluginId,capability,input:{prompt:$('#idea').value,seed:Number($('#seed').value),density:Number($('#density').value)/100,brightness:Number($('#density').value)/100}});await refreshJobs();});
}
async function continuePiano(){
  await sessionTransition(async()=>{
    await post('/api/jobs',{projectId:project.id,projectRevision:project.revision,pluginId:'eleutherai.aria-mlx',capability:'continue',input:{sourceTrackId:track()?.id,startBar:Number($('#aria-start').value),phraseBars:Number($('#aria-bars').value),maxNewTokens:Number($('#aria-tokens').value),seed:Number($('#seed').value)}});
    await refreshJobs();toast('Aria is preparing a separate piano answer on this Mac');
  });
}
async function generateAudio(){
  await sessionTransition(async()=>{
    await post('/api/jobs',{projectId:project.id,projectRevision:project.revision,pluginId:'google.magenta-small',capability:'generate-audio',input:{prompt:$('#idea').value,seed:Number($('#seed').value),durationSeconds:Number($('#magenta-duration').value),startBar:Number($('#magenta-start').value),guideWithNotes:$('#magenta-guide').checked,sourceTrackId:track()?.id}});
    await refreshJobs();toast('Magenta is preparing a separate audio candidate on this Mac');
  });
}
async function transcribeSelected(){
  await sessionTransition(async()=>{
    const selected=track();if(!selected?.assetId)throw new Error('Select a recorded vocal, imported audio, or a sample track first');
    $('#library-status').textContent='Preparing audio for local transcription…';
    await audio.ready();const buffer=await audio.loadAsset(selected.assetId);if(buffer.duration>120)throw new Error('Choose a take shorter than two minutes');
    const window=selected.kind==='audio'&&selected.audio?analysisWindow(buffer,selected.audio):undefined;
    const response=await fetch('/api/assets',{method:'POST',headers:{'Content-Type':'audio/wav'},body:bufferWAV(buffer,window)}),result=await response.json();if(!response.ok)throw new Error(result.error);
    await post('/api/jobs',{projectId:project.id,projectRevision:project.revision,pluginId:'spotify.basic-pitch',capability:'transcribe',input:{assetId:result.assetId,sourceAssetId:selected.assetId,sourceTrackId:selected.id,...(window?{analysisWindow:window}:{})}});await refreshJobs();
  });
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
      if(j.result?.tracks?.some(t=>t.notes.length||t.kind==='audio'))action('▶ Audition',()=>startPlayback(candidateAudition(project,j.result),'audition'));
      if(j.result?.assetId)action('▶ Sample',async()=>{await audio.ready();const buffer=await audio.loadAsset(j.result.assetId);audio.voice(60,.8,'sample',audio.context.currentTime,2,buffer);});
      action('Accept',()=>sessionTransition(async()=>{const accepted=await post('/api/jobs/'+j.id+'/accept',{expectedRevision:project.revision});remember();project=accepted.project;selectTrack(project.tracks.at(-1)?.id);await refreshJobs();toast('Added to your session');}),true);
      action('Discard',async()=>{panic();await post('/api/jobs/'+j.id+'/reject',{});await refreshJobs();});
    }else if(['queued','running'].includes(j.state))action('Cancel',async()=>{await post('/api/jobs/'+j.id+'/cancel',{});await refreshJobs();});
    card.append(actions);box.append(card);
  }
}
async function refreshJobs(){const id=project.id,next=await api('/api/jobs?projectId='+id);if(project.id===id&&JSON.stringify(next)!==JSON.stringify(jobs)){jobs=next;renderJobs();}}
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
const capture=new TakeCapture({
  getStream:async(context,signal)=>{
    if(context.withSong)await audio.ready();
    if(signal.aborted)throw new Error('Recording canceled');
    return navigator.mediaDevices.getUserMedia({audio:{echoCancellation:false,noiseSuppression:false,autoGainControl:false}});
  },
  createRecorder:stream=>{const supported=['audio/webm;codecs=opus','audio/mp4','audio/ogg;codecs=opus'].find(type=>MediaRecorder.isTypeSupported(type));return new MediaRecorder(stream,supported?{mimeType:supported}:{});},
  prepare:async(context,signal)=>{
    if(!context.withSong)return;
    if(signal.aborted)throw new Error('Recording canceled');
    signal.addEventListener('abort',()=>{input.panic('voice-take-finished');audio.stop();},{once:true});
    await startPlayback(project,'session',{captureStart:true});
  },
  atStart:context=>context.withSong?{recordingTiming:recordingTiming(audio.project,audio.clock,audio.context.currentTime)}:{},
  persist:persistTake,
  onState:updateCaptureControls,
  onError:error=>{toast(error.message,true);refreshRecovery().catch(()=>{});}
});
function updateCaptureControls(){
  const phase=capture.phase,button=$('#record');
  const labels={idle:'● Record voice',requesting:'Cancel mic request',preparing:'Cancel song preparation',recording:'■ Finish take',stopping:'Finishing take…',saving:'Saving original…',failed:'Retry saving take'};
  button.textContent=labels[phase];button.classList.toggle('recording',phase==='recording');button.disabled=sessionBusy||['stopping','saving'].includes(phase);
  for(const id of ['#project-picker','#new-project','#import-project','#export-project','#import-audio','#import-example'])$(id).disabled=sessionBusy||capture.busy;
  $('#voice-with-song').disabled=sessionBusy||capture.busy;$('#audio-start-bar').disabled=sessionBusy||capture.busy;
  $('#tempo').disabled=sessionBusy||capture.busy;$('#count-in').disabled=sessionBusy||capture.busy||audio.playing;
  $('#download-take').classList.toggle('hidden',phase!=='failed'||!capture.blob);
  $('#cancel-import').classList.toggle('hidden',phase!=='failed'||capture.take?.context.origin!=='import');
  const hints={idle:$('#voice-with-song').checked?'Voice starts with the song from bar 1 after permission. Use headphones; placement is estimated. Imports still use the chosen bar.':'Solo voice and imports use the chosen bar. Originals stay on this Mac · up to 2 minutes and 30 MiB.',requesting:'Waiting for microphone permission. You can cancel this request.',preparing:'Microphone ready. Loading the song before starting the take…',recording:capture.take?.context.withSong?'Recording with the song. Count-in stays in the full original; clip alignment is an uncalibrated estimate.':'Recording your original solo take · up to 2 minutes.',stopping:'Finishing audio and releasing the microphone…',saving:'Checking and saving the original audio with this session…',failed:'Original retained in this tab. Retry saving or download it before closing.'};
  $('#capture-status').textContent=phase==='failed'&&capture.take?.context.recoverySaved?'Original saved in local recovery. Retry now, or recover it after reloading.':hints[phase];
  $$('.recover-take').forEach(button=>button.disabled=sessionBusy||capture.busy);
}
async function refreshRecovery(){
  const box=$('#recovery-list'),entries=await api('/api/pending-takes');box.replaceChildren();$('#take-recovery').classList.toggle('hidden',!entries.length);
  $('#recovery-status').textContent=`${entries.length} unfinished save${entries.length===1?'':'s'}. Originals stay on this Mac. Active recordings are protected only after their completed audio is stored.`;
  for(const entry of entries){
    const row=el('div','recovery-entry'),details=el('div');details.append(el('strong','',entry.track?.name??'Unreadable recovery entry'),el('p','',`${entry.projectName??'Original session'} · ${entry.message}`));row.append(details);
    if(['ready','already-attached'].includes(entry.state)){
      const button=el('button','quiet small recover-take',entry.state==='already-attached'?'Finish recovery':'Open session and recover');button.disabled=sessionBusy||capture.busy;
      button.onclick=()=>recoverTake(entry).catch(error=>toast(error.message,true));row.append(button);
    }
    if(entry.track&&['ready','already-attached','identity-conflict','session-missing'].includes(entry.state)){
      const link=el('a','original-audio','Download retained original');link.href='/api/assets/'+entry.track.assetId;link.download=entry.track.audio.originalName??('OmniuSong-retained.'+entry.track.assetId.split('.').at(-1));row.append(link);
    }
    box.append(row);
  }
}
async function recoverTake(entry){
  if(capture.busy||sessionBusy)throw new Error('Finish the current audio operation first');
  await sessionTransition(async()=>{
    await useProject(await api('/api/projects/'+entry.projectId));
    const response=await fetch('/api/assets/'+entry.track.assetId);if(!response.ok)throw new Error('The retained original is unavailable; recovery metadata was kept');
    const blob=await response.blob(),audioInfo=entry.track.audio;
    await capture.recover(blob,{...structuredClone(audioInfo),projectId:entry.projectId,takeId:entry.id,assetId:entry.track.assetId,assetUploaded:true,proposedTrack:entry.track,recoverySaved:true});
  });
}
function importMetadata(blob){
  return new Promise((resolve,reject)=>{
    const player=new Audio(),url=URL.createObjectURL(blob);
    const finish=(error,duration)=>{clearTimeout(timer);player.onloadedmetadata=null;player.onerror=null;player.removeAttribute('src');player.load();URL.revokeObjectURL(url);error?reject(error):resolve(duration);};
    const timer=setTimeout(()=>finish(new Error('Could not read this audio. Try WAV, MP3, M4A, or FLAC.')),10000);
    player.preload='metadata';player.onloadedmetadata=()=>{const duration=player.duration;if(!Number.isFinite(duration)||duration<=0||duration>120)finish(new Error('Choose audio with a readable duration of no more than two minutes'));else finish(null,duration);};
    player.onerror=()=>finish(new Error('This browser could not read that audio format'));player.src=url;
  });
}
async function persistTake(blob,context){
  if(project.id!==context.projectId)throw new Error('The original is retained, but its session is no longer open');
  if(blob.size>AUDIO_LIMIT_BYTES)throw new Error('The original exceeds 30 MiB. Download it to keep the complete recording.');
  const mimeType=audioType(context.originalName,blob.type);
  if(context.origin==='import'&&!context.durationSeconds){
    await importMetadata(blob);await audio.ready();const decoded=await audio.context.decodeAudioData(await blob.arrayBuffer());
    if(!Number.isFinite(decoded.duration)||decoded.duration<=0||decoded.duration>120)throw new Error('Choose audio no longer than two minutes');
    context.durationSeconds=decoded.duration;
  }
  context.assetId??=await takeAssetId(blob,mimeType);
  if(!context.proposedTrack){
    context.proposedTrack={id:context.takeId,name:context.origin==='import'?(context.originalName.replace(/\.[^.]+$/,'').slice(0,150)||'Imported audio'):'Your voice · take '+(project.tracks.filter(t=>t.kind==='audio').length+1),kind:'audio',instrument:'voice',gain:.7,muted:false,locked:false,notes:[],assetId:context.assetId,
      audio:{origin:context.origin,mimeType,durationSeconds:context.durationSeconds,byteLength:blob.size,...takePlacement(context),...(context.originalName?{originalName:context.originalName}:{}),...(context.startedAt?{startedAt:context.startedAt}:{}),...(context.interruption?{interruption:context.interruption}:{}),...(context.example?{example:true,method:'procedural four-tone fixture'}:{})}};
  }
  await api('/api/pending-takes/'+context.takeId,{method:'PUT',body:JSON.stringify({projectId:context.projectId,track:context.proposedTrack})});
  if(!context.assetUploaded){
    const response=await fetch('/api/assets',{method:'POST',headers:{'Content-Type':mimeType},body:blob}),result=await response.json();
    if(!response.ok)throw new Error(result.error);if(result.assetId!==context.assetId)throw new Error('Original checksum changed; retained the recovery entry');context.assetUploaded=true;
  }
  context.recoverySaved=true;updateCaptureControls();
  const existing=project.tracks.find(t=>t.id===context.takeId);
  if(existing&&!sameTakeSource(existing,context.proposedTrack))throw new Error('A different track uses this take identity. Download the retained original before reviewing the session.');
  if(!existing){
    if(project.tracks.length>=64)throw new Error('This session already has 64 tracks. Download the original before making room.');
    remember();const t=structuredClone(context.proposedTrack);
    const view=clipWindow(t.audio),endTick=view.startTick+view.durationSeconds*project.tempo/60*project.ppq;
    project.bars=Math.max(project.bars,Math.min(128,Math.ceil(endTick/3840)));project.tracks.push(t);selectedId=t.id;changed();
  }
  await flushSaves();await post('/api/pending-takes/'+context.takeId+'/complete',{});await refreshRecovery();toast(context.origin==='import'?'Original audio imported and saved locally':'Original vocal take saved locally');
}
async function recordVoice(){
  if(capture.phase==='failed'){await capture.retry();return;}
  if(capture.busy){capture.stop();return;}
  if(sessionBusy)throw new Error('Wait for this session to finish opening');
  if(!navigator.mediaDevices?.getUserMedia||!window.MediaRecorder)throw new Error('Microphone capture is unavailable here. Open the local studio in Chrome.');
  if(project.tracks.length>=64)throw new Error('This session already has 64 tracks');
  const withSong=$('#voice-with-song').checked,startTick=withSong?0:newAudioStart();panic();
  await capture.start({projectId:project.id,takeId:crypto.randomUUID(),origin:'microphone',withSong,startTick});
}
function newAudioStart(){const bar=Number($('#audio-start-bar').value);if(!Number.isInteger(bar)||bar<1||bar>128)throw new Error('Choose a starting bar from 1 to 128');return (bar-1)*3840;}
async function importAudio(file){
  if(!file)return;if(sessionBusy)throw new Error('Wait for the session to open');
  audioType(file.name,file.type);
  await capture.import(file,{projectId:project.id,takeId:crypto.randomUUID(),originalName:file.name.slice(0,512),startTick:newAudioStart()});
}
function recordPacket(packet,source,receivedTimeMs,hostTime){
  const t=track();if(sessionBusy||!audio.recordingReady()||!t?.armed||t.locked||t.kind!=='notes')return;
  const event={...packet,...source,receivedTimeMs,tick:Math.floor(audio.tick()),trackId:t.id};
  if(hostTime!==undefined)event.hostTime=hostTime;
  const retained=appendCapture(project,event);changed(false);
  if(!retained&&project.inputCapture.droppedEvents===1)toast('Raw MIDI capture is full. Notes still record; back up this session and start a new one for more raw events.',true);
}
function receiveMIDI(event,source){
  if(!event||sessionBusy||!project)return;
  const addressed={...event,...source};
  if(event.kind==='on')noteOn(event.pitch,event.velocity16/65535,source.sourceId,addressed);
  if(event.kind==='off')input.noteOff(addressed);
  if(event.kind==='control')input.control(addressed);
}
window.omniuNative=message=>{
  if(message.type==='status')$('#midi-status').textContent=message.message;
  if(message.type==='midi-reset'){for(const sourceId of nativeSources)input.releaseSource(sourceId,'connection-reset');nativeSources.clear();}
  if(message.type==='ump')try{
    const source={sourceId:'coremidi:'+message.sourceId,sourceName:(message.sourceName??'CoreMIDI input').slice(0,160)},receivedTimeMs=performance.now();nativeSources.add(source.sourceId);
    for(const packet of decodeUMP(message.words)){
      recordPacket({format:'ump',words:packet.words},source,receivedTimeMs,message.hostTime);
      receiveMIDI(packet.event,source);
    }
  }catch(error){toast(error.message,true);}
};
async function connectMIDI(){
  await audio.ready();
  if(window.webkit?.messageHandlers?.omniu){window.webkit.messageHandlers.omniu.postMessage({action:'connectMIDI'});return;}
  if(!navigator.requestMIDIAccess)throw new Error('Use the Mac app or Chrome for MIDI input. The on-screen keys are ready here.');
  midiAccess??=await navigator.requestMIDIAccess({sysex:false});
  const bind=()=>{
    const next=new Map([...midiAccess.inputs.values()].filter(port=>port.state==='connected').map(port=>['webmidi:'+port.id,port]));
    for(const [id,port]of midiConnected)if(!next.has(id)){port.onmidimessage=null;input.releaseSource(id);}
    for(const [sourceId,port]of next)port.onmidimessage=e=>{
      try{const source={sourceId,sourceName:(port.name??'MIDI input').slice(0,160)};recordPacket({format:'midi1',bytes:Array.from(e.data)},source,e.timeStamp);receiveMIDI(decodeMIDI1(e.data),source);}catch(error){toast(error.message,true);}
    };
    midiConnected.clear();for(const [id,port]of next)midiConnected.set(id,port);$('#midi-status').textContent=`${next.size} MIDI 1 input(s) · Web MIDI`;
  };bind();midiAccess.onstatechange=bind;
}
async function boot(){
  [catalog,plugins,selections]=await Promise.all([api('/api/catalog'),api('/api/plugins'),api('/api/selections')]);
  const projects=await refreshLibrary();let lastId;try{lastId=localStorage.getItem('omniusong.activeProject');}catch{}project=await api('/api/projects/'+(projects.find(p=>p.id===lastId)??projects[0]).id);selectedId=project.tracks[0]?.id;$('#project-picker').value=project.id;
  for(const category of [...new Set(catalog.models.map(m=>m.category))]){const option=el('option','',category);option.value=category;$('#model-category').append(option);}
  makePerformance();renderStudio();renderModels();updateCaptureControls();await refreshJobs();
  await refreshRecovery().catch(()=>{$('#take-recovery').classList.remove('hidden');$('#recovery-status').textContent='Local audio recovery could not be checked. Use Refresh recovery before closing an unsaved take.';});
  $$('.rail-button').forEach(b=>b.onclick=()=>{$$('.rail-button').forEach(x=>x.classList.toggle('active',x===b));$$('.view').forEach(v=>v.classList.toggle('hidden',v.id!==b.dataset.view+'-view'));});
  const guarded=(selector,fn)=>{$(selector).onclick=()=>Promise.resolve().then(()=>{
    const stopAction=selector==='#panic'||selector==='#play'&&audio.playing||selector==='#record'&&capture.busy;
    if(stopAction||commitPendingEdits())return fn();
  }).catch(e=>toast(e.message,true));};
  guarded('#refresh-recovery',refreshRecovery);
  $('#voice-with-song').onchange=updateCaptureControls;
  guarded('#save',save);guarded('#play',async()=>{if(audio.playing)panic();else await startPlayback();});guarded('#panic',async()=>panic());
  guarded('#sketch',()=>generate('local.sketch','compose'));guarded('#instrument',()=>generate('local.instrument','instrument'));guarded('#lyrics',()=>generate($('#use-ollama').checked?'ollama.lyrics':'local.lyric-cards','lyrics'));guarded('#transcribe',transcribeSelected);guarded('#record',recordVoice);guarded('#midi-connect',connectMIDI);
  guarded('#metronome',()=>changeTransport({metronome:!transportSettings(project.transport).metronome}));
  $('#count-in').onchange=()=>{try{if(commitPendingEdits())changeTransport({countInBars:Number($('#count-in').value)});}catch(error){toast(error.message,true);renderTransportControls();}};
  $('#click-level').onchange=()=>{try{if(commitPendingEdits())changeTransport({clickLevel:Number($('#click-level').value)/100});}catch(error){toast(error.message,true);renderTransportControls();}};
  guarded('#aria-continue',continuePiano);
  guarded('#magenta-generate',generateAudio);
  guarded('#roll-prev',async()=>{rollPage--;renderRoll();});guarded('#roll-next',async()=>{rollPage++;renderRoll();});
  $('#roll-page').onchange=()=>{rollPage=Number($('#roll-page').value);renderRoll();};$('#roll-pitch').onchange=()=>{rollTop=Number($('#roll-pitch').value);renderRoll();};
  guarded('#undo',async()=>{if(!undoStack.length)return;panic();const revision=project.revision;project=undoStack.pop();project.revision=revision;selectedId=track()?.id;locateSelectedNote();changed();});
  guarded('#clear-track',async()=>{if(track()?.locked)return;panic();remember();track().notes=[];selectedNoteId=null;changed();});
  $('#note-picker').onchange=()=>selectNote($('#note-picker').value);
  for(const field of $$('[data-note-field]')){
    field.oninput=()=>{if(!selectedNote())return;if(noteDraft?.noteId!==selectedNoteId||noteDraft?.trackId!==track()?.id)noteDraft={noteId:selectedNoteId,trackId:track()?.id,patch:{}};noteDraft.patch[field.dataset.noteField]=field.value;$('#note-edit-status').textContent='Save note or press Enter. Leaving this note also saves.';$('#save-status').textContent='Note edit pending';};
  }
  $('#note-inspector').onsubmit=e=>{e.preventDefault();commitPendingEdits();};
  guarded('#note-restore',()=>transformSelectedNote(restoreNote));guarded('#note-delete',()=>transformSelectedNote(deleteNote));
  for(const field of $$('[data-clip-field]'))field.oninput=()=>{if(!track()?.audio)return;if(clipDraft?.trackId!==track().id)clipDraft={trackId:track().id,patch:{}};clipDraft.patch[field.dataset.clipField]=field.value;$('#clip-edit-status').textContent='Save clip or press Enter. Changing tracks also saves.';$('#save-status').textContent='Clip edit pending';};
  $('#clip-inspector').onsubmit=e=>{e.preventDefault();commitPendingEdits();};
  guarded('#clip-restore',()=>{panic();const next=restoreClip(project,track()?.id);if(next!==project){remember();project=next;changed();}});
  guarded('#clip-preview',async()=>{const t=track(),preview=candidateAudition(project,{tracks:[t],auditionStartTick:t.audio.startTick??0});for(const copy of preview.tracks)copy.muted=false;await startPlayback(preview,'audition');});
  guarded('#add-track',async()=>{remember();const t={id:crypto.randomUUID(),name:'Instrument '+(project.tracks.length+1),kind:'notes',instrument:'keys',gain:.65,muted:false,locked:false,notes:[]};project.tracks.push(t);selectedId=t.id;selectedNoteId=null;rollTop=null;rollPage=0;changed();});
  guarded('#new-project',()=>sessionTransition(async()=>useProject(await post('/api/projects',{name:'Untitled session'}))));
  $('#project-picker').onchange=()=>{const id=$('#project-picker').value;sessionTransition(async()=>useProject(await api('/api/projects/'+id))).catch(e=>{$('#project-picker').value=project.id;toast(e.message,true);});};
  guarded('#import-project',async()=>{if(capture.busy)throw new Error('Finish saving the original audio before restoring a backup');$('#archive-file').click();});
  guarded('#import-audio',async()=>{if(capture.busy||sessionBusy)throw new Error('Finish the current audio operation first');$('#audio-file').click();});
  $('#audio-file').onchange=async()=>{try{await importAudio($('#audio-file').files[0]);}catch(error){toast(error.message,true);}finally{$('#audio-file').value='';}};
  guarded('#import-example',async()=>{if(capture.busy||sessionBusy)throw new Error('Finish the current audio operation first');const projectId=project.id,startTick=newAudioStart(),response=await fetch('/api/examples/four-tones.wav');if(!response.ok)throw new Error('Could not load the example');const blob=await response.blob();if(project.id!==projectId)throw new Error('Session changed; choose Import example phrase again');await capture.import(blob,{projectId,takeId:crypto.randomUUID(),originalName:'Example four-tone phrase.wav',example:true,startTick});});
  guarded('#download-take',async()=>{if(capture.blob){const mime=capture.blob.type||'application/octet-stream',extension=Object.entries({wav:'audio/wav',webm:'audio/webm',mp4:'audio/mp4',ogg:'audio/ogg',mp3:'audio/mpeg',flac:'audio/flac'}).find(([,type])=>type===mime)?.[0]??'bin';download(capture.blob,capture.take.context.originalName??('OmniuSong-original.'+extension),mime);}});
  guarded('#cancel-import',async()=>capture.cancelImport());
  $('#archive-file').onchange=async()=>{const file=$('#archive-file').files[0];try{await restoreBackup(file);}catch(error){toast(error.message,true);}finally{$('#archive-file').value='';}};
  $('#project-name').onfocus=()=>remember();
  $('#project-name').oninput=()=>{project.name=$('#project-name').value.trim()||'Untitled session';changed(false);};
  $('#tempo').onchange=()=>{const value=Number($('#tempo').value);if(value<30||value>300){toast('Choose a tempo from 30 to 300 BPM',true);$('#tempo').value=project.tempo;return;}remember();panic();project.tempo=value;changed();};
  $('#lyric-editor').oninput=()=>{if(project.lyricsLocked)return;project.lyrics=$('#lyric-editor').value;changed(false);};$('#lock-lyrics').onchange=()=>{remember();project.lyricsLocked=$('#lock-lyrics').checked;changed();};
  guarded('#export-project',()=>sessionTransition(async()=>{
    $('#library-status').textContent='Packing your music and audio…';const response=await fetch('/api/projects/'+project.id+'/archive');
    if(!response.ok){const error=await response.json();throw new Error(error.error);}
    download(await response.blob(),(project.name.replace(/[^\p{L}\p{N} -]/gu,'').trim()||'OmniuSong')+'.omniusong','application/vnd.omniusong.project+gzip');toast('Complete project backup saved with its referenced audio');
  }));
  guarded('#export-midi',async()=>{panic();await flushSaves();const response=await fetch('/api/projects/'+project.id+'/midi');if(!response.ok)throw new Error('MIDI export failed');download(await response.blob(),'omniusong.mid','audio/midi');const warning=decodeURIComponent(response.headers.get('X-OmniuSong-Warnings')??'');toast(warning||'MIDI 1 exported. Your full expression data stays in the project.');});
  guarded('#export-ump',async()=>{panic();await flushSaves();download(await api('/api/projects/'+project.id+'/ump'),'omniusong.ump.json');toast('MIDI 2 UMP packet trace exported. Standard Clip File export is planned.');});
  $('#model-search').oninput=renderModels;$('#model-category').onchange=renderModels;
  const keys={KeyA:60,KeyS:62,KeyD:64,KeyF:65,KeyG:67,KeyH:69,KeyJ:71,KeyK:72},pressed=new Map();
  window.addEventListener('keydown',e=>{
    if(e.repeat||/INPUT|TEXTAREA|SELECT|AUDIO|VIDEO/.test(e.target.tagName)||e.metaKey||e.ctrlKey||e.altKey)return;
    if(e.code==='Space'&&e.target.closest('button,summary,a[href]'))return;
    if(keys[e.code]!==undefined){e.preventDefault();pressed.set(e.code,noteOn(keys[e.code],.7,'computer'));}
    if(e.code==='Space'){e.preventDefault();$('#play').click();}
  });
  window.addEventListener('keyup',e=>{const voiceId=pressed.get(e.code);if(voiceId)noteOff(keys[e.code],'computer',voiceId);pressed.delete(e.code);});
  const releaseFocus=()=>{pressed.clear();input.panic('focus-lost');};
  window.addEventListener('blur',releaseFocus);document.addEventListener('visibilitychange',()=>{if(document.hidden)releaseFocus();});
  guarded('#sustain',async()=>setLocalSustain($('#sustain').getAttribute('aria-pressed')!=='true'));
  window.addEventListener('beforeunload',e=>{if(dirty||capture.busy||sessionBusy||noteFieldPending()||clipFieldPending()){e.preventDefault();e.returnValue='';}});
  setInterval(()=>{if(!document.hidden&&!dirty&&!savePromise&&!sessionBusy)refreshJobs().catch(()=>{});},1500);
}
boot().then(()=>{document.body.inert=false;$('#save-status').textContent='Saved on this Mac';}).catch(e=>{document.body.inert=false;toast('Could not open studio: '+e.message,true);});
