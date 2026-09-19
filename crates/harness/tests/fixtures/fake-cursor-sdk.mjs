// Persistent hostile SDK fixture: failures deliberately leave activeRunId set.
// Recovery must use the public store API, retain checkpoints, and not replay.
import fs from 'node:fs';
import path from 'node:path';
function isUncheckpointed(prompt) {
  const marker = prompt.indexOf('\n{"interruptedUserMessages":');
  const current = marker < 0 ? prompt : JSON.parse(prompt.slice(marker + 1)).currentUserMessage;
  return current.startsWith('uncheckpointed-');
}
export class JsonlLocalAgentStore {
  constructor(dir) {
    fs.mkdirSync(dir, {recursive:true});
    const file=path.join(dir,'fixture.json');
    const read=()=>fs.existsSync(file)?JSON.parse(fs.readFileSync(file,'utf8')):{agent:null,runs:{}};
    const write=(data)=>fs.writeFileSync(file,JSON.stringify(data));
    this.userMessages=()=> {
      const log=path.join(dir,'prompts.ndjson');
      return fs.existsSync(log)?fs.readFileSync(log,'utf8').trim().split('\n').filter(Boolean).map(JSON.parse).filter(p=>!isUncheckpointed(p)):[];
    };
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
    async send(prompt,{onDelta,mode}) {
      if (prompt.startsWith("verify-mode:")) {
        if (mode !== prompt.slice("verify-mode:".length)) throw new Error("incorrect conversation mode");
      }
      store.logPrompt(prompt);
      const doc=await store.agents.get({});
      if(doc.activeRunId) throw new Error('Agent already has active run');
      const runId='run-'+Date.now();
      const run={runId,agentId:doc.agentId,status:'running',latestCheckpointRef:checkpoint};
      await store.runs.update({run});
      await store.agents.update({agent:{...doc,status:'running',activeRunId:runId}});
      if(isUncheckpointed(prompt))throw new Error('cancelled before checkpoint');
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
          if(prompt==='incident-auth')return {status:'error',requestId:'request-incident',error:{message:'Authentication error If you are logged in, try logging out and back in.',code:'unauthenticated',cause:{apiKey:'DO-NOT-LOG'}}};
          if(['background-auth','background-throw'].includes(prompt)) {
            setTimeout(()=>{
              const error=Object.assign(new Error('Authentication error'),{code:'unauthenticated',requestId:'request-background',cause:{apiKey:'DO-NOT-LOG'}});
              if(prompt==='background-throw')throw error;
              void Promise.reject(error);
            },0);
            return pending;
          }
          if(prompt==='auth-error')return {status:'error',error:{message:'ERROR_NOT_LOGGED_IN'}};
          if(['hang','hung-cancel'].includes(prompt))return pending;
          await finish('finished');return {status:'finished'};
        },
      };
    },
  };
}
function startupLimit() {
  const file = new URL('../../../startup-limit.json', import.meta.url);
  if (!fs.existsSync(file)) return;
  const state = JSON.parse(fs.readFileSync(file, 'utf8'));
  state.attempts++;
  fs.writeFileSync(file, JSON.stringify(state));
  if (state.attempts <= state.failures) throw new Error(state.message);
}
export const Agent={
  messages:{list:async(_id,{store,limit,offset,cwd,runtime})=>{if(runtime!=='local'||cwd!==(await store.agents.get({})).cwd)throw new Error('history must use the owning workspace');return store.userMessages().slice(offset,offset+limit).map((text,i)=>({type:'user',message:i%3===0?{agentConversationTurn:{user_message:{text}}}:i%3===1?{agentConversationTurn:{userMessage:{text}}}:{turn:{case:'agentConversationTurn',value:{userMessage:{text}}}}}));}},
  async create({local,model}) {
    if (model?.params?.some(p => p.id === "agentMode")) throw new Error("mode leaked into model parameters");
    startupLimit();
    await local.store.agents.create({agent:{agentId:'agent-fixture',cwd:local.cwd,status:'idle',activeRunId:null,latestCheckpoint:checkpoint,sdkMetadata:{mustKeep:true}}});
    return instance(local.store);
  },
  async resume(id,{local}) {
    startupLimit();
    const doc=await local.store.agents.get({agentId:id});
    if(doc.latestCheckpoint.rootBlobId!==checkpoint.rootBlobId || !doc.sdkMetadata.mustKeep)throw new Error('conversation history was lost');
    return instance(local.store);
  },
};
