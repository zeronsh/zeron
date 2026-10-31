import assert from 'node:assert/strict';
import {appendFileSync, mkdirSync, readFileSync, rmSync} from 'node:fs';
import {createInterface} from 'node:readline';
import {dirname, join} from 'node:path';

const mark = process.env.ZERON_PI_SPAWN_MARK ?? join(process.env.HOME, 'Documents/dev/tmp/zeron-perf/scratch/h12-pi-lazy-mcp', `pi-mcp-spawns-${process.pid}`);

if (process.argv[2] === 'server') {
  if (process.env.ZERON_PI_SPAWN_MARK) appendFileSync(process.env.ZERON_PI_SPAWN_MARK, `${process.pid}\n`);
  createInterface({input: process.stdin}).on('line', line => {
    const msg = JSON.parse(line);
    let result;
    if (msg.method === 'initialize' && process.env.ZERON_PI_FAIL_INIT === '1' && readFileSync(process.env.ZERON_PI_SPAWN_MARK, 'utf8').trim().split('\n').length === 1) {
      process.stdout.write(JSON.stringify({jsonrpc:'2.0', id:msg.id, error:{code:-32000,message:'initialization failed'}})+'\n');
      return;
    }
    if (msg.method === 'initialize') result = {protocolVersion: '2024-11-05', capabilities: {tools:{}}, serverInfo: {name:'test',version:'1'}};
    else if (msg.method === 'tools/list') result = {tools:['whoami','create_chat','create_chats','read_chat','send_message','wait_for_turn'].map(name => ({name,description:'Zeron delegation',inputSchema:{type:'object',properties:{mode:{type:'string'}}}}))};
    else if (msg.method === 'tools/call') {
      if (msg.params.arguments.mode === 'hang') return;
      if (msg.params.arguments.mode === 'crash') process.exit(1);
      result = {content:[{type:'text',text:process.env.ZERON_CHAT_ID}], isError:msg.params.arguments.mode === 'error'};
    } else return;
    process.stdout.write(JSON.stringify({jsonrpc:'2.0', id:msg.id, result})+'\n');
  });
} else {
  mkdirSync(dirname(mark), {recursive: true});
  rmSync(mark, {force: true});
  process.env.ZERON_PI_SPAWN_MARK = mark;
  const spawns = () => {
    try { return readFileSync(mark, 'utf8').split('\n').filter(Boolean).length; }
    catch { return 0; }
  };
  const catalog = JSON.parse(readFileSync(new URL('../../src/pi/mcp-tools.json', import.meta.url), 'utf8'));
  const {default: extension} = await import(process.argv[2]);
  let expected = 0;
  for (const chat of ['first', 'second']) {
    process.env.ZERON_PI_MCP = JSON.stringify({name:'zeron',command:process.execPath,args:[process.argv[1],'server'],env:{ZERON_CHAT_ID:chat}});
    const handlers = {}, tools = {};
    extension({on:(event,fn)=>handlers[event]=fn,registerTool:tool=>tools[tool.name]=tool});
    try {
      await handlers.session_start();
      assert.equal(spawns(), expected, 'session start must not start zeron mcp');
      const whoami = catalog.tools.find(tool => tool.name === 'whoami');
      assert.deepEqual(tools.zeron_whoami.parameters, whoami.inputSchema);
      assert.equal(tools.zeron_whoami.description, whoami.description);
      const visual = catalog.tools.find(tool => tool.name === 'show_visual');
      assert.equal(tools.zeron_show_visual.description, visual.description);
      assert.equal(tools.zeron_show_image.name, 'zeron_show_image');
      const concurrent = await Promise.all(['whoami', 'list_devices', 'list_projects'].map(name => tools[`zeron_${name}`].execute('concurrent', {})));
      assert.equal(concurrent.length, 3);
      expected += 1;
      assert.equal(spawns(), expected, 'concurrent first calls share one server');
      for (const name of ['create_chat','create_chats','read_chat','send_message','wait_for_turn']) {
        assert.equal((await tools[`zeron_${name}`].execute('delegation',{})).content[0].text, chat);
      }
      assert.equal(spawns(), expected, 'later calls reuse the server');
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
      assert.equal((await tool.execute('retry',{})).content[0].text, chat);
      expected += 1;
      assert.equal(spawns(), expected, 'a later call reconnects after a crash');
      assert.equal(handlers.message_end, undefined, "native RPC reports provider failures directly");
      // Restarting the session must not let an old child's exit fail new RPCs.
      await handlers.session_start();
      assert.equal(spawns(), expected, 'session start must not start zeron mcp');
      assert.equal((await tools.zeron_whoami.execute('call',{})).content[0].text,chat);
      expected += 1;
      assert.equal(spawns(), expected, 'the first call of a new session starts the server');
    } finally { handlers.session_shutdown(); }
    await assert.rejects(tools.zeron_whoami.execute('call',{}),/not connected/);
    assert.equal(spawns(), expected, 'a call after shutdown must not start the server');
  }
  process.env.ZERON_PI_MCP = JSON.stringify({name:'zeron',command:process.execPath,args:[process.argv[1],'server'],env:{}});
  const handlers = {}, tools = {};
  extension({on:(event,fn)=>handlers[event]=fn,registerTool:tool=>tools[tool.name]=tool});
  try {
    await handlers.session_start();
    assert.equal(tools.zeron_show_visual, undefined);
    assert.equal(tools.zeron_show_image, undefined);
    assert.equal(tools.zeron_whoami.description, catalog.tools.find(tool => tool.name === 'whoami').description);
    assert.equal(spawns(), expected);
    await tools.zeron_whoami.execute('call',{});
    expected += 1;
    assert.equal(spawns(), expected, 'the first call starts the server outside a chat too');
  } finally { handlers.session_shutdown(); }
  rmSync(mark, {force: true});
  process.env.ZERON_PI_MCP = JSON.stringify({name:'zeron',command:process.execPath,args:[process.argv[1],'server'],env:{ZERON_CHAT_ID:'retry',ZERON_PI_FAIL_INIT:'1'}});
  const retryHandlers = {}, retryTools = {};
  extension({on:(event,fn)=>retryHandlers[event]=fn,registerTool:tool=>retryTools[tool.name]=tool});
  await retryHandlers.session_start();
  await assert.rejects(retryTools.zeron_whoami.execute('first',{}), /initialization failed/);
  assert.equal((await retryTools.zeron_whoami.execute('second',{})).content[0].text, 'retry');
  assert.equal(spawns(), 2, 'a later call reconnects after initialization fails');
  retryHandlers.session_shutdown();
  process.env.ZERON_CHAT_ID = 'inherited';
  process.env.ZERON_PI_MCP = JSON.stringify({name:'zeron',command:process.execPath,args:[process.argv[1],'server'],env:{}});
  const inherited = {}, inheritedTools = {};
  extension({on:(event,fn)=>inherited[event]=fn,registerTool:tool=>inheritedTools[tool.name]=tool});
  await inherited.session_start();
  assert.ok(inheritedTools.zeron_show_visual, 'catalog must follow the inherited server environment');
  inherited.session_shutdown();
  delete process.env.ZERON_CHAT_ID;
  rmSync(mark, {force: true});
}
