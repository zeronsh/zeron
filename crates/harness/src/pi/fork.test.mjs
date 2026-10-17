import { test } from 'node:test';
import { writeFileSync } from 'node:fs';
import assert from 'node:assert/strict';
import { mkdtemp, writeFile, readFile, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { pathToFileURL } from 'node:url';
import { execute } from './fork.mjs';
import nativePoint from './native-point.mjs';

test('native entry notification uses the exact persisted object, not repeated text', () => {
  let handler;
  const notices = [];
  nativePoint({ on: (event, fn) => { assert.equal(event, 'turn_end'); handler = fn; } });
  const message = { role: 'assistant', stopReason: 'stop', content: [{type:'text', text:'same'}] };
  let entry = { type: 'message', id: 'entry-a', message };
  const ctx = { sessionManager: { getLeafEntry: () => entry, getSessionId: () => 'source' },
    ui: { notify: (message) => notices.push(message) } };
  handler({ message: structuredClone(message) }, ctx);
  assert.equal(notices.length, 0);
  handler({ message }, ctx);
  assert.deepEqual(JSON.parse(notices[0].split('zeron-native-fork-v1:')[1]), {sessionId:'source', entryId:'entry-a'});
  for (const stopReason of ['toolUse', 'aborted', 'error', 'length']) {
    message.stopReason = stopReason; handler({ message }, ctx);
  }
  message.stopReason = 'stop';
  entry = {type:'custom', id:'later'};
  handler({message}, ctx);
  assert.equal(notices.length, 1);
});

async function fixture(t) {
  const dir = await mkdtemp(join(tmpdir(), 'pi fork with spaces '));
  t.after(() => rm(dir, { recursive:true, force:true }));
  const sourceFile = join(dir, 'source.jsonl');
  const sourceSessionId = 'source';
  await writeFile(sourceFile, JSON.stringify({type:'session',version:3,id:sourceSessionId,cwd:dir})+'\n');
  return {dir, sourceFile, sourceSessionId, entryId:'a1'};
}

test('invalid identity, cwd, format and missing boundary never invoke branching', async t => {
  const request = await fixture(t);
  let opens = 0, creates = 0;
  const sdk = {CURRENT_SESSION_VERSION:3, SessionManager:{open: () => {
    opens++;
    return {getSessionId:()=> 'source', getEntry:()=> undefined, createBranchedSession:()=> { creates++; }};
  }}};
  for (const overrides of [{sourceSessionId:'other'}, {dir:tmpdir()}, {entryId:'missing'}]) {
    await assert.rejects(execute({...request, ...overrides}, sdk));
  }
  assert.equal(opens, 1); assert.equal(creates, 0);
  await writeFile(request.sourceFile, JSON.stringify({type:'session',version:2,id:'source',cwd:request.dir})+'\n');
  await assert.rejects(execute(request, sdk));
  assert.equal(opens, 1, 'old source must not be opened/migrated');
});

test('provider creation followed by extra history is indeterminate, never accepted', async t => {
  const request = await fixture(t);
  const reply = {type:'message',id:'a1',parentId:null,message:{role:'assistant',stopReason:'stop',content:[]}};
  const childFile = join(request.dir, 'child.jsonl');
  await writeFile(childFile, JSON.stringify({type:'session',version:3,id:'child',cwd:request.dir})+'\n');
  let creates = 0;
  const sdk = {CURRENT_SESSION_VERSION:3, buildSessionContext:()=>({messages:[]}), SessionManager:{open: file => file === request.sourceFile
    ? {getSessionId:()=> 'source', getEntry:()=> reply, getBranch:()=> [reply], getEntries:()=> [reply],
       createBranchedSession:()=> {creates++; return childFile;}}
    : {getSessionId:()=> 'child', getCwd:()=>request.dir, getLeafId:()=> 'a2', getEntries:()=> [reply, {...reply,id:'a2'}]}}};
  await assert.rejects(execute(request, sdk), e => e.indeterminate === true && /prefix/.test(e.message));
  assert.equal(creates, 1);
});

test('strict resume checks do not branch, migrate or accept an unrelated project', async t => {
  const request = await fixture(t);
  let opened = 0;
  const sdk = {CURRENT_SESSION_VERSION:3, SessionManager:{open: () => {
    opened++; return {getSessionId:()=> 'source',getEntries:()=>[{type:'message',message:{role:'assistant',stopReason:'stop'}}]};
  }}};
  assert.deepEqual(await execute({...request,mode:'check'}, sdk), {sessionId:'source',sessionFile:request.sourceFile});
  await assert.rejects(execute({...request,mode:'check',dir:tmpdir()},sdk));
  assert.equal(opened, 1);
  sdk.SessionManager.open = () => ({getSessionId:()=> 'source',getEntries:()=>[]});
  await assert.rejects(execute({...request,mode:'check'},sdk),/history is missing/);
});

test('failure after creating a native file is indeterminate and leaves the source untouched', async t => {
  const request = await fixture(t);
  const before = await readFile(request.sourceFile);
  const reply = {type:'message',id:'a1',parentId:null,message:{role:'assistant',stopReason:'stop',content:[]}};
  const orphan = join(request.dir,'own-child.jsonl');
  let created = false;
  const sdk = {CURRENT_SESSION_VERSION:3,buildSessionContext:()=>({messages:[]}),SessionManager:{open:()=>({
    getSessionId:()=> 'source',getEntry:()=> reply,getBranch:()=>[reply],getEntries:()=>[reply],
    createBranchedSession:()=> { writeFileSync(orphan,'owned child'); created = true; throw new Error('lost child result after creation'); }
  })}};
  await assert.rejects(execute(request,sdk),e=>e.indeterminate === true);
  assert(created);
  assert.equal(await readFile(orphan,'utf8'),'owned child');
  assert.deepEqual(await readFile(request.sourceFile),before);
});

// Set PI_FORK_SDK_MODULE to the session-manager.js of the pinned 0.85.1 SDK.
// These storage tests run with no agent/model runtime, credentials or inference.
const storage = process.env.PI_FORK_SDK_MODULE
  ? await import(pathToFileURL(process.env.PI_FORK_SDK_MODULE).href) : null;
test('official SDK: inclusive historical and final cuts, provenance, context and unchanged source', {skip:!storage}, async t => {
  const request = await fixture(t);
  const source = storage.SessionManager.create(request.dir, request.dir);
  const message = (role, text) => role === 'user'
    ? {role,content:text,timestamp:1}
    : {role,content:[{type:'text',text}],api:'test',provider:'test',model:'test',usage:{input:1,output:1,cacheRead:0,cacheWrite:0,totalTokens:2,cost:{input:0,output:0,cacheRead:0,cacheWrite:0,total:0}},stopReason:'stop',timestamp:1};
  source.appendMessage(message('user', 'U1'));
  const a1 = source.appendMessage(message('assistant','A1'));
  source.appendMessage(message('user','U2'));
  source.appendMessage(message('assistant','A2'));
  source.appendMessage(message('user','U3'));
  const a3 = source.appendMessage(message('assistant','A3'));
  const native = {...request,sourceSessionId:source.getSessionId(),sourceFile:source.getSessionFile()};
  const before = await readFile(native.sourceFile);
  for (const [entryId, count] of [[a1,2],[a3,6]]) {
    const result = await execute({...native,entryId}, storage);
    assert.notEqual(result.sessionId, native.sourceSessionId);
    const child = storage.SessionManager.open(result.sessionFile, request.dir);
    assert.equal(child.getEntries().length, count);
    assert.equal(child.getLeafId(), entryId);
    assert.deepEqual(storage.buildSessionContext(child.getEntries(),child.getLeafId()), storage.buildSessionContext(source.getEntries(),entryId));
    assert.deepEqual(await readFile(native.sourceFile), before);
    // A fork of the fork still resolves its own exact entry; the original ID
    // remains a valid canonical source for inherited visual messages.
    const grandchild = await execute({...native,sourceSessionId:result.sessionId,sourceFile:result.sessionFile,entryId:a1}, storage);
    assert.equal(storage.SessionManager.open(grandchild.sessionFile,request.dir).getEntries().length,2);
  }
  assert.deepEqual(await readFile(native.sourceFile), before);
});

test('official SDK: labels, compaction and custom state preserve native context', {skip:!storage}, async t => {
  const request = await fixture(t);
  const source = storage.SessionManager.create(request.dir, request.dir);
  const user = source.appendMessage({role:'user',content:'U1',timestamp:1});
  source.appendMessage({role:'assistant',content:[{type:'text',text:'A1'}],stopReason:'stop',timestamp:1});
  source.appendLabelChange(user, 'start');
  const label = source.getLeafId();
  source.appendMessage({role:'user',content:'U2',timestamp:2});
  source.appendCompaction('summary of U1/A1', label, 100);
  source.appendCustomEntry('probe-state', {keep:true});
  const a2 = source.appendMessage({role:'assistant',content:[{type:'text',text:'A2'}],stopReason:'stop',timestamp:2});
  const before = await readFile(source.getSessionFile());
  const result = await execute({...request,sourceFile:source.getSessionFile(),sourceSessionId:source.getSessionId(),entryId:a2},storage);
  const child = storage.SessionManager.open(result.sessionFile,request.dir);
  assert.deepEqual(child.buildSessionContext(),source.buildSessionContext());
  assert(child.getEntries().some(e=>e.type==='custom' && e.customType==='probe-state'));
  assert.deepEqual(await readFile(source.getSessionFile()), before);
});
