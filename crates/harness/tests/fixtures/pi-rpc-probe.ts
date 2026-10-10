import { appendFileSync, existsSync } from "node:fs";
import { createAssistantMessageEventStream } from "@earendil-works/pi-ai";
export default function(pi) {
  pi.registerTool({name:'native_probe_tool',label:'Native fork probe',description:'Local test tool',
    parameters:{type:'object',properties:{},required:[]},
    execute:async () => ({content:[{type:'text',text:'local tool result'}],details:{}})});
  pi.on('input', event => {
    if (/^(burst-|late-)/.test(event.text)) {
      // Queue acceptance finishes in microtasks after this handler returns.
      // Report it in the next timer phase while the mock model stays blocked.
      setTimeout(() => appendFileSync('probe-inputs.jsonl', JSON.stringify(event.text)+'\n'), 0);
    }
    return { action: 'continue' };
  });
  pi.registerCommand('probe-new', {description:'New native session', handler: async (_args, ctx) => { await ctx.newSession(); }});
  pi.registerCommand('probe-metadata', {description:'Native extension state', handler: async () => { pi.appendEntry('probe-state', {saved: true}); }});
  pi.registerCommand('probe-noop', {description:'No model run', handler: async () => {}});
  pi.registerCommand('probe-input', {description:'Input dialog', handler: async (_args, ctx) => { const value = await ctx.ui.input('Probe input'); ctx.ui.notify(`answer:${value}`, 'info'); }});
  pi.registerProvider('zeron-probe', {
    baseUrl:'http://127.0.0.1:1', apiKey:'local-fake-key', api:'zeron-probe-api',
    models:[{id:'mock', name:'Local mock', reasoning:true, input:['text'], cost:{input:0,output:0,cacheRead:0,cacheWrite:0}, contextWindow:128000,maxTokens:4096}],
    streamSimple(model, context, options) {
      const stream = createAssistantMessageEventStream();
      const last = [...context.messages].reverse().find(m => m.role === 'user');
      const text = typeof last?.content === 'string' ? last.content : (last?.content || []).filter(c=>c.type==='text').map(c=>c.text).join('');
      const batch = [];
      for (let i = context.messages.length - 1; i >= 0 && context.messages[i].role === 'user'; i--) {
        const content = context.messages[i].content;
        batch.unshift(typeof content === 'string' ? content : content.filter(c=>c.type==='text').map(c=>c.text).join(''));
      }
      const gate = text === 'burst hold' ? 'initial' : text.startsWith('burst-') ? 'burst' : text.startsWith('late-') ? 'late' : undefined;
      if (gate) appendFileSync('probe-model-calls.jsonl', JSON.stringify(batch)+'\n');
      const msg = {role:'assistant',content:[], api:model.api, provider:model.provider, model:model.id, usage:{input:10,output:2,cacheRead:0,cacheWrite:0,totalTokens:12,cost:{input:0,output:0,cacheRead:0,cacheWrite:0,total:0}},stopReason:'stop',timestamp:Date.now()};
      (async () => {
        stream.push({type:'start',partial:msg});
        if (text === 'native-tool-loop' && context.messages.at(-1)?.role !== 'toolResult') {
          msg.stopReason = 'toolUse';
          msg.content.push({type:'toolCall',id:'native-tool-call',name:'native_probe_tool',arguments:{}});
          stream.push({type:'toolcall_start',contentIndex:0,partial:msg});
          stream.push({type:'toolcall_end',contentIndex:0,toolCall:msg.content[0],partial:msg});
          stream.push({type:'done',reason:'toolUse',message:msg});stream.end();return;
        }
        while (gate && !existsSync('probe-release-'+gate) && !options?.signal?.aborted) {
          await new Promise(resolve => setTimeout(resolve, 5));
        }
        await new Promise(resolve => {const t=setTimeout(resolve, text.includes('slow')?500:20);options?.signal?.addEventListener('abort',()=>{clearTimeout(t);resolve();},{once:true});});
        if (options?.signal?.aborted || text === 'error') {
          msg.stopReason = options?.signal?.aborted ? 'aborted' : 'error';msg.errorMessage = 'local mock failure';
          stream.push({type:'error',reason:msg.stopReason,error:msg});stream.end();return;
        }
        msg.content.push({type:'text',text:''});
        stream.push({type:'text_start',contentIndex:0,partial:msg});
        msg.content[0].text = text === 'fork-context'
          ? JSON.stringify(context.messages.map(m => ({role: m.role, content: m.content})))
          : 'MOCK:'+(gate ? batch.join('|') : text);
        stream.push({type:'text_delta',contentIndex:0,delta:msg.content[0].text,partial:msg});
        stream.push({type:'text_end',contentIndex:0,content:msg.content[0].text,partial:msg});
        stream.push({type:'done',reason:'stop',message:msg});stream.end();
      })();
      return stream;
    }
  });
}
