const fields={pitch:[0,127],startTick:[0,128*3840-1],durationTicks:[1,128*3840],velocity16:[0,65535]};
const musicalValues=note=>Object.fromEntries(Object.keys(fields).map(key=>[key,note[key]]));
function selected(project,trackId,noteId){
  const track=project.tracks.find(t=>t.id===trackId);
  if(!track||track.kind!=='notes')throw new Error('Choose a note track');
  if(track.locked)throw new Error('Unlock this track before editing notes');
  const note=track.notes.find(n=>n.id===noteId);
  if(!note)throw new Error('Choose a note first');
  if(project.history.length>=5000)throw new Error('This session has reached its edit history limit');
  return note;
}
function check(values,expression){
  for(const [key,[min,max]]of Object.entries(fields))if(!Number.isInteger(values[key])||values[key]<min||values[key]>max)throw new Error(`${key==='velocity16'?'Velocity':key==='startTick'?'Start tick':key==='durationTicks'?'Length in ticks':'Pitch'} must be a whole number from ${min} to ${max}`);
  if(values.startTick+values.durationTicks>128*3840)throw new Error('The note must end within 128 bars');
  const tail=(expression??[]).reduce((last,lane)=>lane.points.reduce((tick,p)=>Math.max(tick,p.tick),last),0);
  if(values.durationTicks<tail)throw new Error(`Keep at least ${tail} ticks to preserve this note’s expression`);
}
function transaction(project,trackId,noteId,action,operation){
  const next=structuredClone(project),track=next.tracks.find(t=>t.id===trackId),index=track.notes.findIndex(n=>n.id===noteId);
  const detail=operation(track,index);
  next.history.push({id:crypto.randomUUID(),at:new Date().toISOString(),action,trackId,noteId,...detail});
  return next;
}
export function editNote(project,trackId,noteId,patch){
  const original=selected(project,trackId,noteId);
  if(!patch||Object.keys(patch).some(key=>!Object.hasOwn(fields,key)))throw new Error('Only pitch, timing, and velocity can be changed here');
  const before=musicalValues(original),after={...before,...patch};check(after,original.expression);
  if(Object.keys(fields).every(key=>before[key]===after[key]))return project;
  const next=transaction(project,trackId,noteId,'edit-note',(track,index)=>{
    const note=track.notes[index];
    note.editSource??={version:1,...before,expression:structuredClone(note.expression??[])};
    Object.assign(note,after);return {before,after};
  });
  next.bars=Math.max(next.bars,Math.ceil((after.startTick+after.durationTicks)/3840));
  return next;
}
export function restoreNote(project,trackId,noteId){
  const note=selected(project,trackId,noteId),source=note.editSource;
  if(!source)return project;
  if(source.version!==1)throw new Error('Unsupported original note snapshot');
  check(source,source.expression);
  const next=transaction(project,trackId,noteId,'restore-note',(track,index)=>{
    Object.assign(track.notes[index],musicalValues(source),{expression:structuredClone(source.expression??[])});
    delete track.notes[index].editSource;return {before:musicalValues(note),after:musicalValues(source)};
  });
  next.bars=Math.max(next.bars,Math.ceil((source.startTick+source.durationTicks)/3840));return next;
}
export function deleteNote(project,trackId,noteId){
  selected(project,trackId,noteId);
  return transaction(project,trackId,noteId,'delete-note',(track,index)=>({removedNote:track.notes.splice(index,1)[0]}));
}
