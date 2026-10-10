// Run with CLAUDE_FORK_TEST_SDK pointing at the pinned SDK's sdk.mjs.
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { mkdtemp, mkdir, writeFile, rm, readFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { pathToFileURL } from 'node:url';
import { randomUUID } from 'node:crypto';
import { execute } from './fork.mjs';

test('pinned SDK storage fork respects CLAUDE_CONFIG_DIR and remaps UUIDs', { skip: !process.env.CLAUDE_FORK_TEST_SDK }, async () => {
  const root = await mkdtemp(join(tmpdir(), 'zeron claude fork '));
  const previous = process.env.CLAUDE_CONFIG_DIR;
  try {
    process.env.CLAUDE_CONFIG_DIR = join(root,'config');
    const sdk = await import(pathToFileURL(process.env.CLAUDE_FORK_TEST_SDK).href);
    const dir = join(root,'project with spaces');
    await mkdir(dir);
    const sessionId = randomUUID();
    const ids = Array.from({length:4}, () => randomUUID());
    const project = join(process.env.CLAUDE_CONFIG_DIR, 'projects', dir.replace(/[^a-zA-Z0-9]/g, '-'));
    await mkdir(project, {recursive:true});
    const file = join(project, sessionId + '.jsonl');
    const lines = ['user','assistant','user','assistant'].map((type,i) => JSON.stringify({type, uuid:ids[i], parentUuid:ids[i-1] ?? null, sessionId, cwd:dir, isSidechain:false, userType:'external', version:'2.1.284', timestamp:new Date().toISOString(), message:{role:type,content:[{type:'text',text:i<2?'initial context':'later secret'}]}})).join('\n') + '\n';
    await writeFile(file,lines);
    const source = await sdk.getSessionMessages(sessionId, {dir});
    assert.equal(source.length,4);
    const child = await execute({sourceSessionId:sessionId,dir,upToMessageId:ids[1]},sdk);
    assert.notEqual(child.sessionId,sessionId);
    const copied = await sdk.getSessionMessages(child.sessionId,{dir});
    assert.equal(copied.length,2);
    assert.notEqual(copied[1].uuid,ids[1]);
    assert.equal(await readFile(file,'utf8'),lines);
  } finally {
    if (previous === undefined) delete process.env.CLAUDE_CONFIG_DIR; else process.env.CLAUDE_CONFIG_DIR = previous;
    await rm(root,{recursive:true,force:true});
  }
});
