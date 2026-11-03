/**
 * Per-chat gate for runner bearers on chat-scoped data — `/chat2/{chatId}/*`
 * (transcripts, command rows a host executes) and `/blob/{chatId}/*` (tool
 * outputs): a runner may touch a chat only while the registry says its device
 * hosts it (`chats/{chatId}` live, `fields.deviceId === dev`; a subagent's
 * doc goes by the chat that spawned it). Without this a
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

/** A subagent's transcript doc is `{chatId}--sub--{toolUseId}` (nested ones
 * are named after the root chat too) and has no registry row of its own: it
 * belongs to whoever hosts the chat that spawned it. */
export const gatedChat = (chatId: string): string => chatId.split("--sub--", 1)[0] || chatId;

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
  const chat = gatedChat(chatId);
  const key = `${auth.orgId}/${auth.userId}/${auth.deviceId}/${chat}`;
  const until = allowed.get(key);
  if (until !== undefined && until > now) return undefined;
  allowed.delete(key);

  const room = env.REGISTRY_ROOMS.get(env.REGISTRY_ROOMS.idFromName(`reg1/${auth.orgId}/${auth.userId}`));
  let host: unknown;
  let parentHost: unknown;
  try {
    const reply = await room.fetch(`https://registry/chat-host?chat=${encodeURIComponent(chat)}`, {
      headers: { [AUTH_USER_HEADER]: auth.userId }
    });
    if (!reply.ok) throw new Error(`registry ${reply.status}`);
    ({ deviceId: host, parentHost } = (await reply.json()) as { deviceId?: unknown; parentHost?: unknown });
  } catch (e) {
    console.warn("runner chat gate: registry unavailable", String(e));
    return json({ error: "unavailable", message: "Couldn't check the chat's host; retry." }, 503);
  }
  // Its own chat, or a delegate it started on another machine (it hosts the
  // delegate's parent).
  if (host !== auth.deviceId && parentHost !== auth.deviceId) return notHost();
  if (allowed.size >= MAX_CACHE) allowed.clear();
  allowed.set(key, now + POSITIVE_TTL_MS);
  return undefined;
};

const deviceAllowed = new Map<string, number>();

/**
 * Gate for a runner on ANOTHER device's room (`/device/{id}/*` — that
 * device's RPC surface: queues, interrupts, files). Allowed only when the
 * device hosts a chat whose parent the runner hosts: the runner started that
 * delegate and manages it. Its own room and read-only liveness never reach
 * here (runner-policy.ts). Cached like the chat gate: positive answers only.
 */
export const runnerDeviceGate = async (
  env: Pick<Env, "REGISTRY_ROOMS">,
  auth: Verified,
  deviceId: string,
  now = Date.now()
): Promise<Response | undefined> => {
  if (auth.kind !== "runner" || deviceId === auth.deviceId) return undefined;
  const refused = () => json({ error: "forbidden", message: "A runner may only reach the machines of chats it started." }, 403);
  if (!auth.orgId || !auth.deviceId) return refused();
  const key = `${auth.orgId}/${auth.userId}/${auth.deviceId}/${deviceId}`;
  const until = deviceAllowed.get(key);
  if (until !== undefined && until > now) return undefined;
  deviceAllowed.delete(key);

  const room = env.REGISTRY_ROOMS.get(env.REGISTRY_ROOMS.idFromName(`reg1/${auth.orgId}/${auth.userId}`));
  let hosts: unknown;
  try {
    const reply = await room.fetch(`https://registry/device-parents?device=${encodeURIComponent(deviceId)}`, {
      headers: { [AUTH_USER_HEADER]: auth.userId }
    });
    if (!reply.ok) throw new Error(`registry ${reply.status}`);
    hosts = ((await reply.json()) as { hosts?: unknown }).hosts;
  } catch (e) {
    console.warn("runner device gate: registry unavailable", String(e));
    return json({ error: "unavailable", message: "Couldn't check the device's chats; retry." }, 503);
  }
  if (!Array.isArray(hosts) || !hosts.includes(auth.deviceId)) return refused();
  if (deviceAllowed.size >= MAX_CACHE) deviceAllowed.clear();
  deviceAllowed.set(key, now + POSITIVE_TTL_MS);
  return undefined;
};
