// Optional real-Pi test. Requires PI_CODING_AGENT_PACKAGE, makes no model/API requests.
import assert from 'node:assert/strict';
import {spawn} from 'node:child_process';
import {mkdirSync,mkdtempSync,rmSync,writeFileSync} from 'node:fs';
import {tmpdir} from 'node:os';
import {dirname,join,resolve} from 'node:path';

const pkg=process.env.PI_CODING_AGENT_PACKAGE;
assert.ok(pkg,'set PI_CODING_AGENT_PACKAGE to the installed Pi package directory');
const bridge=resolve(process.argv[2]);
const fixtures=dirname(resolve(process.argv[1]));
const dir=mkdtempSync(join(tmpdir(),'zeron-pi-mcp-native-'));
try {
  const agent=join(dir,'agent');
  mkdirSync(agent);
  writeFileSync(join(agent,'settings.json'),'{}');
  const child=spawn(process.execPath,[
    join(pkg,'dist/cli.js'),'--mode','json','--no-session','--no-extensions',
    '--extension',join(fixtures,'pi-mcp-model.ts'),'--extension',bridge,
    '--extension',join(dirname(bridge),'images.mjs'),
    ...(process.env.PI_IMAGE_SHRINK_EXTENSION ? ['--extension',process.env.PI_IMAGE_SHRINK_EXTENSION] : []),
    '--extension',join(pkg,'dist/extensions/codemode/index.js'),
    '--extension',join(pkg,'dist/extensions/tool-search/index.js'),
    '--model','zeron-mcp-fixture/mock','offline-mcp-probe',
  ],{
    cwd:dir,stdio:['ignore','pipe','pipe'],
    env:{...process.env,PI_CODING_AGENT_DIR:agent,ZERON_PI_MCP:JSON.stringify({
      name:'zeron',command:process.execPath,args:[join(fixtures,'pi-mcp-test.mjs'),'server'],
      env:{ZERON_CHAT_ID:'native-probe',TEST_WAIT_ON_BACKGROUND:'1'},
    })},
  });
  let stdout='',stderr='';
  child.stdout.on('data',chunk=>stdout+=chunk);
  child.stderr.on('data',chunk=>stderr+=chunk);
  const timeout=setTimeout(()=>child.kill(),15_000);
  let code;
  try {
    code=await new Promise((resolve,reject)=>{
      child.once('error',reject);child.once('exit',resolve);
    });
  } finally {clearTimeout(timeout);}
  assert.equal(code,0,stderr);
  const events=stdout.trim().split('\n').map(line=>JSON.parse(line));
  const messages=events.filter(event=>event.type==='message_end').map(event=>event.message);
  const final=messages.find(message=>message.role==='assistant'&&message.content.some(block=>
    block.type==='text'&&block.text==='native-pi-mcp:ok'));
  assert.ok(final,JSON.stringify(messages)+'\n'+stderr);
  const shot=messages.find(message=>message.role==='toolResult'&&message.toolCallId==='mcp-screenshot');
  assert.equal(shot.isError,false);
  assert.ok(shot.content.some(block=>block.type==='image'));
  const refused=messages.find(message=>message.role==='toolResult'&&message.toolCallId==='mcp-refusal');
  assert.equal(refused.isError,true);
  assert.ok(refused.content.some(block=>block.type==='text'&&block.text.includes('refused:browser_route_unavailable')));
  console.log('Real Pi: prompt instructions, deferred discovery, codemode screenshot and structured refusal passed');
} finally {rmSync(dir,{recursive:true,force:true});}
