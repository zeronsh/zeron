import { test } from 'node:test';
import assert from 'node:assert/strict';
import { execute } from './fork.mjs';
const message = (type, uuid, text) => ({type, uuid, session_id:'source', message:{role:type,content:[{type:'text',text}]}, parent_tool_use_id:null, parent_agent_id:null});
const history = [message('user','u1','question one'), message('assistant','a1','answer one'), message('user','u2','secret later'), message('assistant','a2','answer two')];

test('inclusive official helper, remapped IDs, correct project, zero inference', async () => {
  const calls = [];
  const sdk = {
    getSessionMessages: async (id, options) => { assert.equal(options.dir, '/project with spaces'); return id === 'source' ? history : history.slice(0,2).map((m,i) => ({...m, uuid:`child-${i}`, session_id:'child'})); },
    forkSession: async (id, options) => { calls.push({id,options}); return {sessionId:'child'}; },
    query: () => assert.fail('inference is forbidden'),
  };
  assert.deepEqual(await execute({sourceSessionId:'source', dir:'/project with spaces', upToMessageId:'a1'}, sdk), {sessionId:'child'});
  assert.deepEqual(calls, [{id:'source', options:{dir:'/project with spaces',upToMessageId:'a1'}}]);
});

test('missing, user, or subagent boundaries fail before creation', async () => {
  const sdk = { getSessionMessages: async () => [...history, {...message('assistant','child','nested'),parent_tool_use_id:'tool'}], forkSession: () => assert.fail('must not create') };
  for (const uuid of [undefined,'missing','u1','child']) {
    await assert.rejects(execute({sourceSessionId:'source',dir:'/project',upToMessageId:uuid}, sdk), e => !e.indeterminate);
  }
});

test('provider ignoring cutoff is indeterminate and never accepted', async () => {
  const sdk = {getSessionMessages: async () => history, forkSession: async () => ({sessionId:'child'})};
  await assert.rejects(execute({sourceSessionId:'source',dir:'/project',upToMessageId:'a1'}, sdk), e => e.indeterminate === true);
});
