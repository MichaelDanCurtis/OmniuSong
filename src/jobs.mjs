import { uid, mergeCandidate, demand, copy } from './project.mjs';
import { Conflict } from './storage.mjs';

export class JobQueue {
  constructor(store,plugins){this.store=store;this.plugins=plugins;this.jobs=new Map();this.active=null;this.acceptTail=Promise.resolve();}
  async init(){
    for(const job of await this.store.list('jobs')){
      if(['queued','running'].includes(job.state)){job.state='interrupted';job.error='Application stopped before completion. Retry as a new job.';await this.store.saveJob(job);}
      this.jobs.set(job.id,job);
    }
    return this;
  }
  async submit(request){
    demand(this.jobs.size<10000,'Job archive is full');
    const manifest=this.plugins.list().find(p=>p.id===request.pluginId);
    demand(manifest&&manifest.capabilities.includes(request.capability),'Unsupported plugin task');
    const project=await this.store.read('projects',request.projectId);
    if(project.revision!==request.projectRevision)throw new Conflict();
    const job={id:uid(),pluginId:request.pluginId,capability:request.capability,projectId:project.id,projectRevision:project.revision,
      input:copy(request.input??{}),state:'queued',decision:'pending',createdAt:new Date().toISOString(),ai:manifest.ai,pluginVersion:manifest.version};
    await this.store.saveJob(job);this.jobs.set(job.id,job);setImmediate(()=>this.drain());return copy(job);
  }
  async drain(){
    if(this.active)return;
    const job=[...this.jobs.values()].find(j=>j.state==='queued');if(!job)return;
    const controller=new AbortController();this.active={id:job.id,controller};
    const timer=setTimeout(()=>controller.abort(new Error('Job exceeded the 5 minute limit')),300000);timer.unref();
    try{
      job.state='running';job.startedAt=new Date().toISOString();await this.store.saveJob(job);
      const project=await this.store.read('projects',job.projectId);
      job.result=await this.plugins.run(job.pluginId,{project:copy(project),input:copy(job.input),capability:job.capability,signal:controller.signal});
      controller.signal.throwIfAborted();job.state='succeeded';
    }catch(error){job.state=job.state==='canceled'?'canceled':'failed';job.error=error.message;delete job.result;}
    finally{clearTimeout(timer);job.finishedAt=new Date().toISOString();await this.store.saveJob(job);this.active=null;setImmediate(()=>this.drain());}
  }
  async cancel(id){
    const job=this.jobs.get(id);demand(job,'Job not found');
    if(!['queued','running'].includes(job.state))return copy(job);
    job.state='canceled';if(this.active?.id===id)this.active.controller.abort(new Error('Canceled by user'));
    await this.store.saveJob(job);return copy(job);
  }
  decide(id,decision,expectedRevision){
    const result=this.acceptTail.then(async()=>{
      const job=this.jobs.get(id);demand(job?.state==='succeeded','Only completed candidates can be reviewed');
      demand(['accepted','rejected'].includes(decision),'Invalid candidate decision');
      if(job.decision!=='pending') {demand(job.decision===decision,'Candidate was already reviewed');return {job:copy(job),project:await this.store.read('projects',job.projectId)};}
      let project=await this.store.read('projects',job.projectId);
      // Read-back of the history also makes a retry safe after a crash between the two file writes.
      if(project.history.some(h=>h.provenance?.jobId===id)){job.decision='accepted';await this.store.saveJob(job);return {job:copy(job),project};}
      if(decision==='accepted'){
        if(project.revision!==expectedRevision||project.revision!==job.projectRevision)throw new Conflict('Project was edited after this candidate started. Generate a fresh candidate to preserve your edits.');
        project=mergeCandidate(project,job.result,{jobId:job.id,pluginId:job.pluginId,pluginVersion:job.pluginVersion,model:job.result.model??null,ai:job.ai,input:job.input});
        project=await this.store.saveProject(project,expectedRevision);
      }
      job.decision=decision;job.reviewedAt=new Date().toISOString();await this.store.saveJob(job);return {job:copy(job),project};
    });
    this.acceptTail=result.catch(()=>{});return result;
  }
  list(projectId){return [...this.jobs.values()].filter(j=>j.projectId===projectId).map(copy).sort((a,b)=>b.createdAt.localeCompare(a.createdAt));}
  async close(){if(this.active)this.active.controller.abort(new Error('Application is closing'));}
}
