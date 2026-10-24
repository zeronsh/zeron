// Official storage APIs only: no AgentSession, prompt, extensions, or query.
import { open, realpath } from 'node:fs/promises';
import { dirname } from 'node:path';
import { pathToFileURL } from 'node:url';

async function header(file) {
  const handle = await open(file, 'r');
  try {
    const bytes = Buffer.alloc(65536);
    const { bytesRead } = await handle.read(bytes, 0, bytes.length, 0);
    const end = bytes.subarray(0, bytesRead).indexOf(10);
    if (end < 0) throw new Error('Invalid Pi session header');
    return JSON.parse(bytes.subarray(0, end).toString('utf8'));
  } finally { await handle.close(); }
}

export async function execute(request, sdk) {
  const { sourceSessionId, sourceFile, dir, entryId, mode = 'fork' } = request;
  if (!sourceSessionId || !sourceFile || !dir) throw new Error('Native Pi session and project are required');
  const sourceHeader = await header(sourceFile);
  // Opening an old format can migrate/rewrite it. Refuse before calling the SDK.
  if (sourceHeader.type !== 'session' || sourceHeader.id !== sourceSessionId
      || sourceHeader.version !== sdk.CURRENT_SESSION_VERSION
      || await realpath(sourceHeader.cwd) !== await realpath(dir)) {
    throw new Error('Native Pi session identity, format, or project does not match');
  }
  const manager = sdk.SessionManager.open(sourceFile, dirname(sourceFile));
  if (manager.getSessionId() !== sourceSessionId) throw new Error('Pi opened an unexpected session');
  if (mode === 'check') {
    if (!manager.getEntries().some(e => e.type === 'message'
        && e.message.role === 'assistant' && e.message.stopReason === 'stop')) {
      throw new Error('Required native Pi fork history is missing');
    }
    return { sessionId: sourceSessionId, sessionFile: sourceFile };
  }
  if (mode !== 'fork' || !entryId) throw new Error('An exact Pi assistant entry is required');
  const entry = manager.getEntry(entryId);
  if (entry?.type !== 'message' || entry.message.role !== 'assistant'
      || entry.message.stopReason !== 'stop') throw new Error('Native Pi reply is not complete or no longer exists');
  const branch = manager.getBranch(entryId);
  if (branch.some((e, i) => e.parentId !== (i ? branch[i - 1].id : null))) {
    throw new Error('Native Pi branch ancestry is incomplete');
  }
  // Label entries are annotations rather than conversation turns. The official
  // SDK re-chains their descendants and remaps compaction references to them.
  const replacements = new Map();
  let labels = [];
  const expected = [];
  for (const item of branch) {
    if (item.type === 'label') { labels.push(item.id); continue; }
    for (const id of labels) replacements.set(id, item.id);
    labels = [];
    expected.push(item.type === 'compaction'
      ? { ...item, firstKeptEntryId: replacements.get(item.firstKeptEntryId) ?? item.firstKeptEntryId }
      : item);
  }
  if (expected.at(-1)?.id !== entryId) throw new Error('Native Pi boundary is unavailable');
  const context = sdk.buildSessionContext(manager.getEntries(), entryId);
  // SDK branching preserves entry IDs, but removes/recreates label annotations.
  const canonical = ({ parentId, ...entry }) => JSON.stringify(entry);
  let creating = false;
  try {
    creating = true;
    const sessionFile = manager.createBranchedSession(entryId);
    if (!sessionFile || sessionFile === sourceFile) throw new Error('Pi did not create an independent session file');
    const childHeader = await header(sessionFile);
    if (childHeader.type !== 'session' || childHeader.version !== sdk.CURRENT_SESSION_VERSION) {
      throw new Error('Pi created an incompatible session file');
    }
    const child = sdk.SessionManager.open(sessionFile, dirname(sessionFile));
    const copied = child.getEntries().filter(e => e.type !== 'label');
    const childContext = sdk.buildSessionContext(child.getEntries(), child.getLeafId());
    if (!child.getSessionId() || child.getSessionId() === sourceSessionId
        || childHeader.id !== child.getSessionId()
        || await realpath(child.getCwd()) !== await realpath(dir)
        || copied.length !== expected.length
        || copied.some((e, i) => e.parentId !== (i ? expected[i - 1].id : null))
        || copied.some((e, i) => canonical(e) !== canonical(expected[i]))
        || JSON.stringify(context) !== JSON.stringify(childContext)) {
      throw new Error('Pi did not preserve the selected native branch prefix');
    }
    // Sync the created native file before the host publishes its saved ID.
    const handle = await open(sessionFile, 'r+');
    try { await handle.sync(); } finally { await handle.close(); }
    if (process.platform !== 'win32') {
      const directory = await open(dirname(sessionFile), 'r');
      try { await directory.sync(); } finally { await directory.close(); }
    }
    return { sessionId: child.getSessionId(), sessionFile };
  } catch (error) {
    error.indeterminate = creating;
    throw error;
  }
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  try {
    let input = '';
    for await (const chunk of process.stdin) {
      input += chunk;
      if (input.length > 65536) throw new Error('Request is too large');
    }
    // The pinned official module avoids importing agent/model runtime services.
    const sdk = await import(new URL('./core/session-manager.js', import.meta.resolve('@earendil-works/pi-coding-agent')));
    process.stdout.write(JSON.stringify({ ok: await execute(JSON.parse(input), sdk) }) + '\n');
  } catch (error) {
    process.stdout.write(JSON.stringify({ error: error.message, indeterminate: Boolean(error.indeterminate) }) + '\n');
    process.exitCode = 1;
  }
}
