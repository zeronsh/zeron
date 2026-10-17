import { test } from 'node:test';
import assert from 'node:assert/strict';
import { execute } from './fork.mjs';
const message = (type, uuid, text) => ({type, uuid, session_id:'source', message:{role:type,content:[{type:'text',text}]}, parent_tool_use_id:null, parent_agent_id:null});
const history = [message('user','u1','question one'), message('assistant','a1','answer one'), message('user','u2','secret later'), message('assistant','a2','answer two')];
const request = {sourceSessionId:'source', dir:'/project with spaces', upToMessageId:'a1'};
const copy = entries => entries.map((m,i) => ({...m, uuid:`child-${i}`, session_id:'child'}));
function fixture({entries = history, child = copy(history.slice(0,2)), current = history} = {}) {
  const calls = [];
  return {
    calls,
    async importSessionToStore(id, store, options) {
      assert.equal(id, 'source');
      assert.deepEqual(options, {dir: request.dir, includeSubagents:false});
      // Match the SDK's batched import rather than assuming a single callback.
      for (let i=0; i<entries.length; i+=2) await store.append({sessionId:id}, entries.slice(i,i+2));
    },
    async getSessionMessages(id, options) {
      assert.equal(options.dir, request.dir);
      assert.equal(options.includeSystemMessages, true);
      if (options.sessionStore) return options.sessionStore.load({sessionId:id});
      return id === 'source' ? current : child;
    },
    async forkSession(id, options) { calls.push({id,options}); return {sessionId:'child'}; },
    query() { assert.fail('inference is forbidden'); },
  };
}

test('inclusive official helper, remapped IDs, correct project, zero inference', async () => {
  const sdk = fixture();
  assert.deepEqual(await execute(request, sdk), {sessionId:'child'});
  assert.deepEqual(sdk.calls, [{id:'source', options:{dir:request.dir,upToMessageId:'a1'}}]);
});

test('missing, user, or subagent boundaries fail before creation', async () => {
  const sdk = fixture({entries:[...history,
    {...message('assistant','nested','nested'),parent_tool_use_id:'tool'},
    {...message('assistant','sidechain','nested'),isSidechain:true},
    {...message('assistant','team','nested'),teamName:'team'},
    {...message('assistant','agent','nested'),parent_agent_id:'agent'},
  ]});
  for (const uuid of [undefined,'missing','u1','nested','sidechain','team','agent']) {
    await assert.rejects(execute({...request,upToMessageId:uuid}, sdk), e => !e.indeterminate);
  }
  assert.equal(sdk.calls.length,0);
});

test('later compaction cannot replace the historical prefix with the current conversation view', async () => {
  const sdk = fixture({entries:[...history, message('system','compact','later summary')], current:[message('system','compact','later summary'),history[1]]});
  assert.deepEqual(await execute(request, sdk), {sessionId:'child'});
});

test('historical boundary no longer visible in the current view remains resolvable by exact UUID', async () => {
  const sdk = fixture({current:[message('system','compact','later summary'),history[3]]});
  assert.deepEqual(await execute(request,sdk),{sessionId:'child'});
});

test('wrong, incomplete, reordered or expanded child prefixes remain indeterminate', async () => {
  for (const child of [copy(history), copy(history.slice(0,1)), copy([history[1],history[0]]), copy([history[0],message('assistant','a1','changed')])]) {
    const sdk = fixture({child});
    await assert.rejects(execute(request,sdk),e=>e.indeterminate === true && e.message === 'Provider did not preserve the selected transcript prefix');
    assert.equal(sdk.calls.length,1);
  }
});

test('snapshot loading failures do not create provider sessions', async () => {
  const sdk = fixture();
  sdk.importSessionToStore = async () => { throw new Error('unreadable session'); };
  await assert.rejects(execute(request,sdk), e=>e.message === 'unreadable session' && !e.indeterminate);
  assert.equal(sdk.calls.length,0);
});

test('resume existence checks use only the official conversation read', async () => {
  const sdk = fixture();
  sdk.importSessionToStore = () => assert.fail('resume checks do not need a snapshot');
  assert.deepEqual(await execute({...request,mode:'check'},sdk),{sessionId:'source'});
  assert.equal(sdk.calls.length,0);
});
