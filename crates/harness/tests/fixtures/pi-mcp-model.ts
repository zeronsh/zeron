// Offline model: verifies the real Pi prompt, codemode images and refused results.
import assert from 'node:assert/strict';
import {createAssistantMessageEventStream} from '@earendil-works/pi-ai';

export default function (pi) {
  pi.registerProvider('zeron-mcp-fixture', {
    baseUrl:'http://127.0.0.1:1',apiKey:'offline-fixture',api:'zeron-mcp-fixture-api',
    models:[{id:'mock',name:'Offline MCP fixture',reasoning:false,input:['text','image'],
      cost:{input:0,output:0,cacheRead:0,cacheWrite:0},contextWindow:128000,maxTokens:1024}],
    streamSimple(model, context) {
      const system = context.messages.filter(message=>message.role==='system');
      const systemText = JSON.stringify(system);
      assert.match(systemText,/Use zeron_computer_run_steps with zeron_computer_get_window_state/);
      assert.match(systemText,/zeron_\* tools are deferred: call via codemode \(tools\.zeron_\*, searchTools, describeTool\) or tool_search\./);
      const declared = system.flatMap(message=>message.toolsAdded??[]);
      const declaredNames = declared.map(tool=>tool.name);
      assert.ok(declaredNames.includes('codemode'),`codemode must be auto-activated: ${declaredNames.join(',')}`);
      assert.ok(declaredNames.includes('zeron_wait_on_background'),`wait_on_background must stay direct: ${declaredNames.join(',')}`);
      assert.ok(!declaredNames.some(name=>name.startsWith('zeron_')&&name!=='zeron_wait_on_background'),
        `deferred zeron tools must stay undeclared: ${declaredNames.join(',')}`);
      const codemodeTool = declared.find(tool=>tool.name==='codemode');
      assert.ok(!codemodeTool.description.includes('zeron_computer_get_window_state'),
        'deferred tools must not be inlined into codemode description');
      const results = context.messages.filter(message=>message.role==='toolResult');
      const stream = createAssistantMessageEventStream();
      const message = {role:'assistant',content:[],api:model.api,provider:model.provider,model:model.id,
        usage:{input:10,output:5,cacheRead:0,cacheWrite:0,totalTokens:15,
          cost:{input:0,output:0,cacheRead:0,cacheWrite:0,total:0}},stopReason:'stop',timestamp:Date.now()};
      let call;
      if (results.length === 0) {
        call = {type:'toolCall',id:'mcp-screenshot',name:'codemode',arguments:{code:
          'const r = await tools.zeron_computer_get_window_state({}); ' +
          'if (r.isError) throw new Error("snapshot failed"); ' +
          'text(r.structuredContent); ' +
          'for (const block of r.content) { if (block.type === "image") image(block); else if (block.type === "text") text(block.text); }'}};
      } else if (results.length === 1) {
        assert.equal(results.at(-1).isError,false);
        assert.ok(results.at(-1).content.some(block=>block.type==='image'),'codemode must forward the image');
        assert.ok(results.at(-1).content.some(block=>block.type==='text'&&block.text.includes('s00000123')));
        call = {type:'toolCall',id:'mcp-refusal',name:'codemode',arguments:{code:
          'const r = await tools.zeron_computer_get_window_state({mode:"refused"}); ' +
          'if (!r.isError) throw new Error("expected refusal"); ' +
          'throw new Error("refused:" + (r.structuredContent?.refusal?.code ?? "missing"));'}};
      } else {
        assert.equal(results.length,2,'the probe must not retry computer input');
        assert.equal(results.at(-1).isError,true,'a structured browser refusal must be an error');
        assert.ok(results.at(-1).content.some(block=>block.type==='text'&&block.text.includes('refused:browser_route_unavailable')),
          'refusal code must propagate through codemode');
      }
      queueMicrotask(()=>{
        stream.push({type:'start',partial:message});
        if (call) {
          message.stopReason='toolUse';message.content.push(call);
          stream.push({type:'toolcall_start',contentIndex:0,partial:message});
          stream.push({type:'toolcall_end',contentIndex:0,toolCall:call,partial:message});
        } else {
          const text='native-pi-mcp:ok';message.content.push({type:'text',text});
          stream.push({type:'text_start',contentIndex:0,partial:message});
          stream.push({type:'text_delta',contentIndex:0,delta:text,partial:message});
          stream.push({type:'text_end',contentIndex:0,content:text,partial:message});
        }
        stream.push({type:'done',reason:message.stopReason,message});stream.end();
      });
      return stream;
    },
  });
}
