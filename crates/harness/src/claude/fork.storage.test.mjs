// Run with CLAUDE_FORK_TEST_SDK pointing at the pinned SDK's sdk.mjs.
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { mkdtemp, mkdir, writeFile, rm, readFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { pathToFileURL } from 'node:url';
import { randomUUID } from 'node:crypto';
import { execute } from './fork.mjs';

test('pinned SDK forks exact historical prefixes before and after compaction without inference', { skip: !process.env.CLAUDE_FORK_TEST_SDK }, async () => {
  const root = await mkdtemp(join(tmpdir(), 'zeron claude fork '));
  const previous = process.env.CLAUDE_CONFIG_DIR;
  try {
    process.env.CLAUDE_CONFIG_DIR = join(root,'config');
    const sdk = await import(pathToFileURL(process.env.CLAUDE_FORK_TEST_SDK).href);
    const dir = join(root,'project with spaces');
    await mkdir(dir);
    const project = join(process.env.CLAUDE_CONFIG_DIR, 'projects', dir.replace(/[^a-zA-Z0-9]/g, '-'));
    await mkdir(project, {recursive:true});
    for (const compact of ['none', 'after-cut', 'before-cut']) {
      const sessionId = randomUUID();
      const ids = Array.from({length:6}, () => randomUUID());
      const common = {sessionId, cwd:dir, isSidechain:false, userType:'external', version:'2.1.284', timestamp:new Date().toISOString()};
      const entries = ['user','assistant','user','assistant','user','assistant'].map((type,i) => ({...common,type,uuid:ids[i],parentUuid:ids[i-1] ?? null,message:{role:type,content:[{type:'text',text:i<4?`historical context ${i}`:'later secret'}]}}));
      if (compact !== 'none') {
        const preserved = compact === 'after-cut' ? ids[3] : ids[1];
        const boundary = randomUUID();
        const summary = randomUUID();
        const cut = compact === 'after-cut' ? 4 : 2;
        entries.splice(cut,0,
          {...common,type:'system',subtype:'compact_boundary',uuid:boundary,parentUuid:null,
            compactMetadata:{trigger:'auto',preTokens:1000,
              preservedSegment:{headUuid:preserved,anchorUuid:summary,tailUuid:preserved},
              preservedMessages:{anchorUuid:summary,uuids:[preserved],allUuids:[preserved]},
            }},
          {...common,type:'user',uuid:summary,parentUuid:boundary,isCompactSummary:true,isVisibleInTranscriptOnly:true,
            message:{role:'user',content:[{type:'text',text:'compacted earlier context'}]}},
        );
      }
      const file = join(project, sessionId + '.jsonl');
      const lines = entries.map(entry=>JSON.stringify(entry)).join('\n')+'\n';
      await writeFile(file,lines);
      const source = await sdk.getSessionMessages(sessionId, {dir,includeSystemMessages:true});
      if (compact === 'after-cut') {
        // The old implementation sees only summary + preserved reply at this
        // boundary, whereas the historical fork must contain all four messages.
        assert.equal(source.findIndex(m=>m.uuid===ids[3]),2);
        assert.equal(source.some(m=>m.uuid===ids[0]),false);
      }
      const calls = [];
      const child = await execute({sourceSessionId:sessionId,dir,upToMessageId:ids[3]}, {
        ...sdk,
        async forkSession(id, options) {
          calls.push({id,options});
          return sdk.forkSession(id,options);
        },
        query() { assert.fail('inference is forbidden'); },
      });
      assert.notEqual(child.sessionId,sessionId);
      assert.deepEqual(calls,[{id:sessionId,options:{dir,upToMessageId:ids[3]}}]);
      const copied = await sdk.getSessionMessages(child.sessionId,{dir,includeSystemMessages:true});
      assert.equal(copied.length,compact === 'before-cut' ? 5 : 4,compact);
      assert.notEqual(copied.at(-1).uuid,ids[3]);
      assert.deepEqual(copied.at(-1).message,entries.find(m=>m.uuid===ids[3]).message);
      assert.equal(JSON.stringify(copied).includes('later secret'),false);
      if (compact === 'after-cut') {
        assert.deepEqual(copied.map(m=>m.message), entries.slice(0,4).map(m=>m.message));
        assert.equal(copied.some(m=>m.type==='system'),false,'no later compact boundary is inherited');
      }
      assert.equal(await readFile(file,'utf8'),lines,'the original session stays byte-for-byte unchanged');
    }
  } finally {
    if (previous === undefined) delete process.env.CLAUDE_CONFIG_DIR; else process.env.CLAUDE_CONFIG_DIR = previous;
    await rm(root,{recursive:true,force:true});
  }
});
