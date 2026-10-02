/**
 * Per-chat gate for runner bearers on chat-scoped data — `/chat2/{chatId}/*`
 * (transcripts, command rows a host executes) and `/blob/{chatId}/*` (tool
 * outputs): a runner may touch a chat only while the registry says its device
 * hosts it (`chats/{chatId}` live, `fields.deviceId === dev`). Without this a
 * stolen runner key could read every transcript or append commands that a
 * LAPTOP host then executes.
 *
 * Positive answers are cached per isolate for ≤5 minutes (a chat's host
 * rarely changes; the registry DO stays off the hot path). Negative answers
 * are never cached: a chat the Cloud device created moments ago must work as
 * soon as its row lands.
 */
import type { Verified } from "./auth";
import { AUTH_USER_HEADER, type Env } from "./env";
import { json } from "./http";

const POSITIVE_TTL_MS = 5 * 60_000;
const MAX_CACHE = 10_000;
const allowed = new Map<string, number>();

/** undefined = proceed; otherwise the refusal to return. */
export const runnerChatGate = async (
  env: Pick<Env, "REGISTRY_ROOMS">,
  auth: Verified,
  chatId: string,
  now = Date.now()
): Promise<Response | undefined> => {
  if (auth.kind !== "runner") return undefined;
  const notHost = () => json({ error: "not_host", message: "This chat is not hosted by this Cloud device." }, 403);
  if (!auth.orgId || !auth.deviceId) return notHost();
  const key = `${auth.orgId}/${auth.userId}/${auth.deviceId}/${chatId}`;
  const until = allowed.get(key);
  if (until !== undefined && until > now) return undefined;
  allowed.delete(key);

  const room = env.REGISTRY_ROOMS.get(env.REGISTRY_ROOMS.idFromName(`reg1/${auth.orgId}/${auth.userId}`));
  let host: unknown;
  try {
    const reply = await room.fetch(`https://registry/chat-host?chat=${encodeURIComponent(chatId)}`, {
      headers: { [AUTH_USER_HEADER]: auth.userId }
    });
    if (!reply.ok) throw new Error(`registry ${reply.status}`);
    host = ((await reply.json()) as { deviceId?: unknown }).deviceId;
  } catch (e) {
    console.warn("runner chat gate: registry unavailable", String(e));
    return json({ error: "unavailable", message: "Couldn't check the chat's host; retry." }, 503);
  }
  if (host !== auth.deviceId) return notHost();
  if (allowed.size >= MAX_CACHE) allowed.clear();
  allowed.set(key, now + POSITIVE_TTL_MS);
  return undefined;
};
