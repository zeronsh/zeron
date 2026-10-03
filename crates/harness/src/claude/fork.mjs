// This module uses only official session storage APIs. It never invokes query().
import { pathToFileURL } from 'node:url';

export async function execute(request, sdk) {
  const { sourceSessionId, dir, upToMessageId, mode = 'fork' } = request;
  if (!sourceSessionId || !dir) throw new Error('Session ID and project directory are required');
  const messages = await sdk.getSessionMessages(sourceSessionId, { dir, includeSystemMessages: true });
  if (!messages.length) throw new Error('Native Claude session no longer exists');
  if (mode === 'check') return { sessionId: sourceSessionId };
  if (mode !== 'fork' || !upToMessageId) throw new Error('An exact assistant UUID is required');
  const boundary = messages.findIndex(m => m.uuid === upToMessageId && m.type === 'assistant' && !m.parent_tool_use_id && !m.parent_agent_id);
  if (boundary < 0) throw new Error('Native assistant UUID is unavailable');
  const expected = messages.slice(0, boundary + 1);
  let created = false;
  try {
    created = true;
    const { sessionId } = await sdk.forkSession(sourceSessionId, { dir, upToMessageId });
    if (!sessionId || sessionId === sourceSessionId) throw new Error('Provider returned no independent session ID');
    const child = await sdk.getSessionMessages(sessionId, { dir, includeSystemMessages: true });
    // The SDK remaps transcript UUIDs. Conversation roles and message payloads
    // must still match exactly, including tool calls/results and system boundaries.
    const canonical = m => JSON.stringify({ type: m.type, message: m.message, parent_tool_use_id: m.parent_tool_use_id ?? null, parent_agent_id: m.parent_agent_id ?? null });
    if (child.length !== expected.length || child.some((m, i) => canonical(m) !== canonical(expected[i]))) {
      throw new Error('Provider did not preserve the selected transcript prefix');
    }
    return { sessionId };
  } catch (error) {
    error.indeterminate = created;
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
    const sdk = await import('@anthropic-ai/claude-agent-sdk');
    const result = await execute(JSON.parse(input), sdk);
    process.stdout.write(JSON.stringify({ ok: result }) + '\n');
  } catch (error) {
    process.stdout.write(JSON.stringify({ error: error.message, indeterminate: Boolean(error.indeterminate) }) + '\n');
    process.exitCode = 1;
  }
}
