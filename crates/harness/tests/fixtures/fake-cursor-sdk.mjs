// Persistent hostile SDK fixture: failures deliberately leave activeRunId set.
// Recovery must use the public store API, retain checkpoints, and not replay.
import fs from 'node:fs';
import path from 'node:path';
export class JsonlLocalAgentStore {
  constructor(dir) {
    fs.mkdirSync(dir, {recursive:true});
    const file=path.join(dir,'fixture.json');
    const read=()=>fs.existsSync(file)?JSON.parse(fs.readFileSync(file,'utf8')):{agent:null,runs:{}};
    const write=(data)=>fs.writeFileSync(file,JSON.stringify(data));
    this.logPrompt=(prompt)=>fs.appendFileSync(path.join(dir,'prompts.ndjson'),JSON.stringify(prompt)+'\n');
    this.agents={
      get:async()=>read().agent,
      create:async({agent})=>{const data=read();data.agent=agent;write(data);return agent;},
      update:async({agent})=>{const data=read();data.agent=agent;write(data);return agent;},
    };
    this.runs={
      get:async({runId})=>read().runs[runId]??null,
      update:async({run})=>{const data=read();data.runs[run.runId]=run;write(data);return run;},
    };
  }
}
export class FileCredentialStore {}
export const Cursor={auth:{status:async()=>({status:'logged-in'})}};
const checkpoint={schemaVersion:1,rootBlobId:'retained-conversation-history'};
function instance(store) {
  return {
    agentId:'agent-fixture',model:{id:'composer-2.5'},close(){},
    async send(prompt,{onDelta}) {
      store.logPrompt(prompt);
      const doc=await store.agents.get({});
      if(doc.activeRunId) throw new Error('Agent already has active run');
      const runId='run-'+Date.now();
      const run={runId,agentId:doc.agentId,status:'running',latestCheckpointRef:checkpoint};
      await store.runs.update({run});
      await store.agents.update({agent:{...doc,status:'running',activeRunId:runId}});
      if(prompt==='send-error') throw new Error('transport failed during send');
      onDelta({update:{type:'text-delta',text:doc.latestCheckpoint.rootBlobId}});
      let resolve;
      const pending=new Promise(r=>resolve=r);
      const finish=async(status)=>{
        await store.runs.update({run:{...run,status}});
        await store.agents.update({agent:{...doc,status:'idle',activeRunId:null}});
        resolve({status});
      };
      return {id:runId,
        async cancel(){if(prompt==='hung-cancel')return new Promise(()=>{});await finish('cancelled');},
        async wait(){
          if(prompt==='wait-error')throw new Error('stream disconnected');
          if(prompt==='auth-error')return {status:'error',error:{message:'ERROR_NOT_LOGGED_IN'}};
          if(['hang','hung-cancel'].includes(prompt))return pending;
          await finish('finished');return {status:'finished'};
        },
      };
    },
  };
}
export const Agent={
  async create({local}) {
    await local.store.agents.create({agent:{agentId:'agent-fixture',cwd:local.cwd,status:'idle',activeRunId:null,latestCheckpoint:checkpoint,sdkMetadata:{mustKeep:true}}});
    return instance(local.store);
  },
  async resume(id,{local}) {
    const doc=await local.store.agents.get({agentId:id});
    if(doc.latestCheckpoint.rootBlobId!==checkpoint.rootBlobId || !doc.sdkMetadata.mustKeep)throw new Error('conversation history was lost');
    return instance(local.store);
  },
};
