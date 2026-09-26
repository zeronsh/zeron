import assert from 'node:assert/strict';
import {createInterface} from 'node:readline';

if (process.argv[2] === 'server') {
  createInterface({input: process.stdin}).on('line', line => {
    const msg = JSON.parse(line);
    let result;
    if (msg.method === 'initialize') result = {protocolVersion: '2024-11-05', capabilities: {tools:{}}, serverInfo: {name:'test',version:'1'}};
    else if (msg.method === 'tools/list') result = {tools:[{name:'whoami',description:'Identity',inputSchema:{type:'object',properties:{mode:{type:'string'}}}}]};
    else if (msg.method === 'tools/call') {
      if (msg.params.arguments.mode === 'hang') return;
      if (msg.params.arguments.mode === 'crash') process.exit(1);
      result = {content:[{type:'text',text:process.env.ZERON_CHAT_ID}], isError:msg.params.arguments.mode === 'error'};
    } else return;
    process.stdout.write(JSON.stringify({jsonrpc:'2.0', id:msg.id, result})+'\n');
  });
} else {
  const {default: extension} = await import(process.argv[2]);
  for (const chat of ['first', 'second']) {
    process.env.ZERON_PI_MCP = JSON.stringify({name:'zeron',command:process.execPath,args:[process.argv[1],'server'],env:{ZERON_CHAT_ID:chat}});
    const handlers = {}, tools = {};
    extension({on:(event,fn)=>handlers[event]=fn,registerTool:tool=>tools[tool.name]=tool});
    try {
      await handlers.session_start();
      const tool = tools.zeron_whoami;
      assert.equal(tool.parameters.type,'object');
      const result = await tool.execute('call',{});
      assert.equal(result.content[0].text,chat);
      await assert.rejects(tool.execute('call',{mode:'error'}), new RegExp(chat));
      const controller = new AbortController();
      const waiting = tool.execute('call',{mode:'hang'},controller.signal);
      controller.abort();
      await assert.rejects(waiting,/cancelled/);
      await assert.rejects(tool.execute('call',{mode:'crash'}),/exited/);
      const notices = [];
      const ctx = {ui:{notify:(message,level)=>notices.push([message,level])}};
      handlers.message_end({message:{role:'assistant',stopReason:'stop',content:[]}}, ctx);
      handlers.message_end({message:{role:'assistant',stopReason:'error',errorMessage:'Codex error: unsupported model'}}, ctx);
      assert.deepEqual(notices,[['Codex error: unsupported model','error']]);
      // Restarting the session must not let an old child's exit fail new RPCs.
      await handlers.session_start();
      assert.equal((await tools.zeron_whoami.execute('call',{})).content[0].text,chat);
    } finally { handlers.session_shutdown(); }
    await assert.rejects(tools.zeron_whoami.execute('call',{}),/not connected/);
  }
}
