import assert from 'node:assert/strict';
import {createInterface} from 'node:readline';

const image = {
  type: 'image', mimeType: 'image/png',
  data: 'iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+aX1sAAAAASUVORK5CYII=',
};
const parameters = {type:'object',properties:{mode:{type:'string'}}};
const annotations = {readOnlyHint:true,destructiveHint:false,idempotentHint:true,openWorldHint:false};
const dataSchema = {type:'object',properties:{answer:{type:'integer'}},required:['answer']};
const chatNames = ['whoami','create_chat','create_chats','read_chat','send_message','wait_for_turn'];
const peerTools = [
  ...chatNames.map(name => ({name,description:'Zeron delegation',inputSchema:parameters})),
  {name:'structured_read',description:'Structured read',inputSchema:parameters,outputSchema:dataSchema,annotations},
  ...['computer_get_window_state','computer_run_steps','computer_help'].map(name => ({
    name,description:'Computer tool',inputSchema:parameters,annotations,
  })),
];
const guidance = 'Use computer_run_steps with computer_get_window_state; computer_* control this Mac. computer_ is the outer prefix; inner step tool: click. computer_browser_* have limits; computer_input_stopped means stop. create_chat delegates.';

if (process.argv[2] === 'server') {
  const cancelled = [];
  createInterface({input: process.stdin}).on('line', line => {
    const msg = JSON.parse(line);
    let result;
    if (msg.method === 'initialize') {
      result = {
        protocolVersion:'2024-11-05',capabilities:{tools:{}},serverInfo:{name:'test',version:'1'},
        ...(process.env.TEST_NO_GUIDANCE === '1' ? {} : {instructions:guidance}),
      };
    } else if (msg.method === 'notifications/cancelled') {
      cancelled.push(msg.params.requestId);
      return;
    } else if (msg.method === 'tools/list') {
      let listed = peerTools;
      if (process.env.TEST_ONLY_WAIT === '1') {
        listed = [{name:'wait_on_background',description:'Wait on background task',inputSchema:parameters}];
      } else {
        if (process.env.TEST_NO_COMPUTER === '1') {
          listed = listed.filter(tool => !tool.name.startsWith('computer_'));
        }
        if (process.env.TEST_WAIT_ON_BACKGROUND === '1') {
          listed = [...listed, {name:'wait_on_background',description:'Wait on background task',inputSchema:parameters}];
        }
      }
      result = {tools:listed};
    } else if (msg.method === 'tools/call') {
      const {name,arguments:args} = msg.params;
      if (args.mode === 'hang') return;
      if (args.mode === 'crash') process.exit(1);
      result = {
        content:[{type:'text',text:args.mode === 'cancelled' ? String(cancelled.length) : process.env.ZERON_CHAT_ID}],
        isError:args.mode === 'error',
      };
      if (name.startsWith('computer_')) {
        result.structuredContent = args.mode === 'refused'
          ? {status:'refused',refusal:{code:'browser_route_unavailable'}}
          : {snapshot_id:'s00000123',pid:1,window_id:7};
        if (name === 'computer_get_window_state' && !result.isError && args.mode !== 'refused') {
          result.content.push(image);
        }
      } else if (name === 'structured_read') {
        result.structuredContent = {answer:42};
      }
    } else return;
    process.stdout.write(JSON.stringify({jsonrpc:'2.0',id:msg.id,result})+'\n');
  });
} else {
  process.setMaxListeners(32);
  const {default: extension} = await import(process.argv[2]);
  for (const modern of [false,true]) for (const chat of ['first','second']) {
    const config = {name:'zeron',command:process.execPath,args:[process.argv[1],'server'],env:{ZERON_CHAT_ID:chat}};
    process.env.ZERON_PI_MCP = JSON.stringify(config);
    const handlers = {}, tools = {};
    extension({
      on:(event,fn)=>handlers[event]=fn,
      registerTool:tool=>tools[tool.name]=tool,
      ...(modern ? {registerMcpServer:()=>{}} : {}),
    });
    try {
      await handlers.session_start();
      assert.deepEqual(Object.keys(tools),peerTools.map(tool=>`zeron_${tool.name}`),'tool names/exposure stay stable');
      assert.ok(Object.values(tools).every(tool=>tool.exposure===undefined),'fallback omits exposure');
      for (const name of chatNames) {
        assert.equal((await tools[`zeron_${name}`].execute('delegation',{})).content[0].text,chat);
      }
      const prompt = {systemPrompt:'Existing instructions',systemPromptOptions:{sections:{existing:'Keep me'}}};
      assert.equal(handlers.before_agent_start(prompt),undefined);
      const section = prompt.systemPromptOptions.sections.zeron_mcp_zeron;
      assert.match(section,/zeron_computer_run_steps/);
      assert.match(section,/zeron_computer_\*/);
      assert.match(section,/zeron_computer_browser_\*/);
      assert.match(section,/zeron_computer_ is the outer prefix/);
      assert.match(section,/inner step tool: click/);
      assert.match(section,/computer_input_stopped/);
      assert.doesNotMatch(section,/zeron_computer_input_stopped/);
      assert.match(section,/zeron_create_chat/);
      assert.equal(prompt.systemPromptOptions.sections.existing,'Keep me');
      handlers.before_agent_start(prompt);
      assert.equal(prompt.systemPromptOptions.sections.zeron_mcp_zeron,section,'guidance is not appended twice');
      const legacy = handlers.before_agent_start({systemPrompt:'Existing instructions'});
      assert.ok(legacy.systemPrompt.startsWith('Existing instructions\n\n'));
      assert.match(legacy.systemPrompt,/zeron_computer_get_window_state/);

      const tool = tools.zeron_whoami;
      assert.deepEqual(tool.parameters,parameters);
      const result = await tool.execute('call',{});
      assert.equal(result.content[0].text,chat);
      assert.equal(result.structuredContent,undefined,'unstructured chat tools keep their text contract');
      await assert.rejects(tool.execute('call',{mode:'error'}),new RegExp(chat));
      assert.deepEqual(tools.zeron_structured_read.outputSchema,dataSchema);
      assert.deepEqual(tools.zeron_structured_read.annotations,annotations);
      assert.deepEqual((await tools.zeron_structured_read.execute('data',{})).structuredContent,{answer:42});

      const state = tools.zeron_computer_get_window_state;
      const snapshot = await state.execute('snapshot',{});
      assert.deepEqual(snapshot.content[1],image,'direct calls keep screenshots');
      assert.equal(snapshot.details.snapshot_id,'s00000123','rendering details keep handles');
      assert.deepEqual(state.annotations,annotations);
      if (modern) {
        assert.equal(state.outputSchema.type,'object');
        assert.equal(snapshot.structuredContent.isError,false);
        assert.equal(snapshot.structuredContent.structuredContent.window_id,7);
        assert.deepEqual(snapshot.structuredContent.content[1],image,'codemode can forward the screenshot');
        assert.equal(state.namespace.name,'zeron');
        assert.match(state.namespace.instructions,/Never print the whole result/);
        for (const mode of ['error','refused']) {
          const failed = await state.execute('failed',{mode});
          assert.equal(failed.isError,true);
          assert.equal(failed.structuredContent.isError,true,'a structured refusal is not success');
          assert.equal(failed.structuredContent.content[0].text,chat);
        }
      } else {
        assert.equal(state.outputSchema,undefined,'old Pi retains its original computer text path');
        assert.equal(snapshot.structuredContent.snapshot_id,'s00000123');
        await assert.rejects(state.execute('failed',{mode:'error'}),new RegExp(chat));
        await assert.rejects(state.execute('refused',{mode:'refused'}),new RegExp(chat));
      }

      const controller = new AbortController();
      const waiting = state.execute('call',{mode:'hang'},controller.signal);
      controller.abort();
      await assert.rejects(waiting,/cancelled/);
      assert.equal((await tool.execute('check',{mode:'cancelled'})).content[0].text,'1','cancellation reaches the MCP peer');
      const preAborted = new AbortController();
      preAborted.abort();
      await assert.rejects(state.execute('call',{},preAborted.signal),/cancelled/);
      await assert.rejects(tool.execute('call',{mode:'crash'}),/exited/);
      assert.equal(handlers.message_end,undefined,'native RPC reports provider failures directly');

      // Restart must neither accept an old peer's replies nor duplicate guidance.
      await handlers.session_start();
      assert.equal((await tools.zeron_whoami.execute('call',{})).content[0].text,chat);
      handlers.before_agent_start(prompt);
      assert.equal(prompt.systemPromptOptions.sections.zeron_mcp_zeron,section);
      process.env.ZERON_PI_MCP = JSON.stringify({...config,env:{...config.env,TEST_NO_GUIDANCE:'1'}});
      // The factory owns its immutable config; a new runtime tests missing server guidance.
      const emptyHandlers = {};
      extension({
        on:(event,fn)=>emptyHandlers[event]=fn,registerTool:()=>{},
        ...(modern ? {registerMcpServer:()=>{}} : {}),
      });
      try {
        await emptyHandlers.session_start();
        emptyHandlers.before_agent_start(prompt);
        assert.doesNotMatch(prompt.systemPromptOptions.sections.zeron_mcp_zeron ?? '',/delegates/,'old guidance does not leak');
      } finally { emptyHandlers.session_shutdown(); }
    } finally {
      handlers.session_shutdown();
    }
    const stoppedPrompt = {systemPromptOptions:{sections:{zeron_mcp_zeron:'stale',existing:'Keep me'}}};
    handlers.before_agent_start(stoppedPrompt);
    assert.deepEqual(stoppedPrompt.systemPromptOptions.sections,{existing:'Keep me'});
    await assert.rejects(tools.zeron_whoami.execute('call',{}),/not connected/);
  }

  async function runDiscoveryCase({name='zeron',env={},modern=true,allTools,initialActive=['read','bash']}) {
    process.env.ZERON_PI_MCP = JSON.stringify({
      name,command:process.execPath,args:[process.argv[1],'server'],env:{ZERON_CHAT_ID:'discovery',...env},
    });
    const handlers = {}, tools = {};
    let active = [...initialActive], setCalls = 0;
    extension({
      on:(event,fn)=>handlers[event]=fn,
      registerTool:tool=>tools[tool.name]=tool,
      ...(modern ? {registerMcpServer:()=>{}} : {}),
      ...(allTools ? {
        getAllTools:()=>allTools,
        getActiveTools:()=>active,
        setActiveTools:next=>{setCalls++;active=[...next];},
      } : {}),
    });
    await handlers.session_start();
    const prompt = {systemPrompt:'Existing',systemPromptOptions:{sections:{}}};
    handlers.before_agent_start(prompt);
    const legacy = handlers.before_agent_start({systemPrompt:'Existing'});
    const sectionName = `zeron_mcp_${name.replace(/[^A-Za-z0-9_]/g,'_')}`;
    return {
      tools,handlers,section:prompt.systemPromptOptions.sections[sectionName],sectionName,legacy,
      active:()=>active,setCalls:()=>setCalls,shutdown:()=>handlers.session_shutdown(),
    };
  }

  // Deferred discovery with codemode + tool_search (function reachesTools as in Pi 1.5.2) and direct wait_on_background.
  {
    const c = await runDiscoveryCase({
      env:{TEST_WAIT_ON_BACKGROUND:'1'},
      allTools:[{name:'read'},{name:'codemode',reachesTools:()=>'all'},{name:'tool_search',reachesTools:()=>'all'}],
    });
    try {
      assert.equal(c.tools.zeron_wait_on_background.exposure,'direct');
      for (const [name,tool] of Object.entries(c.tools)) {
        if (name !== 'zeron_wait_on_background') assert.equal(tool.exposure,'deferred',`${name} must be deferred`);
      }
      assert.equal(c.setCalls(),1);
      assert.deepEqual(c.active(),['read','bash','codemode']);
      assert.match(c.section,/zeron_computer_run_steps/);
      assert.match(c.section,/Never print the whole result/);
      assert.match(c.section,/zeron_\* tools are deferred: call via codemode \(tools\.zeron_\*, searchTools, describeTool\) or tool_search\./);
      assert.match(c.legacy.systemPrompt,/zeron_\* tools are deferred/);
    } finally { c.shutdown(); }
  }

  // Only tool_search reaches tools (fake codemode without reachesTools, empty instructions, no computer_* tools -> extra_mcp).
  {
    const c = await runDiscoveryCase({
      name:'plugin-focus',
      env:{TEST_NO_GUIDANCE:'1',TEST_NO_COMPUTER:'1'},
      allTools:[{name:'codemode'},{name:'tool_search',reachesTools:true}],
    });
    try {
      assert.ok(Object.values(c.tools).every(tool=>tool.exposure==='deferred'));
      assert.equal(c.setCalls(),1);
      assert.deepEqual(c.active(),['read','bash','tool_search']);
      assert.equal(c.section,'plugin-focus_* tools are deferred: call via tool_search.');
    } finally { c.shutdown(); }
    const afterStop = {systemPromptOptions:{sections:{[c.sectionName]:'stale'}}};
    c.handlers.before_agent_start(afterStop);
    assert.deepEqual(afterStop.systemPromptOptions.sections,{});
  }

  // Already-active tool_search stays selected without forcing codemode.
  {
    const c = await runDiscoveryCase({
      allTools:[{name:'codemode',reachesTools:()=>'all'},{name:'tool_search',reachesTools:()=>'all'}],
      initialActive:['read','tool_search'],
    });
    try {
      assert.ok(Object.values(c.tools).every(tool=>tool.exposure==='deferred'));
      assert.equal(c.setCalls(),0);
      assert.deepEqual(c.active(),['read','tool_search']);
    } finally { c.shutdown(); }
  }

  // Server offering only wait_on_background defers nothing and does not force-activate codemode.
  {
    const c = await runDiscoveryCase({
      env:{TEST_ONLY_WAIT:'1'},
      allTools:[{name:'codemode',reachesTools:()=>'all'}],
    });
    try {
      assert.equal(c.tools.zeron_wait_on_background.exposure,'direct');
      assert.equal(c.setCalls(),0);
      assert.deepEqual(c.active(),['read','bash']);
      assert.doesNotMatch(c.section ?? '',/deferred/);
    } finally { c.shutdown(); }
  }

  // Fallback keeps exposure omitted when no active discovery tool reaches tools or !structuredTools.
  for (const opts of [
    {modern:true,allTools:[{name:'read'},{name:'codemode'},{name:'tool_search',reachesTools:false}]},
    {modern:false,allTools:[{name:'codemode',reachesTools:()=>'all'}]},
  ]) {
    const c = await runDiscoveryCase(opts);
    try {
      assert.ok(Object.values(c.tools).every(tool=>tool.exposure===undefined));
      assert.equal(c.setCalls(),0);
      assert.doesNotMatch(c.section,/deferred/);
    } finally { c.shutdown(); }
  }

  console.log('Pi MCP: legacy + structured guidance, deferred discovery, images, refusals, cancellation and restart passed');
}
