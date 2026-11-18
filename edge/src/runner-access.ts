/**
 * The Worker's gate for a runner bearer (a Cloud machine) reaching another
 * chat's data — `/chat2/{chatId}/*` (transcripts, the commands its host
 * runs), `/blob/{chatId}/*` (tool outputs, attachments) — or another
 * device's room as a client (`/device/{id}/ws`, `/nudge`: that device's
 * RPC surface).
 *
 * What it may reach is its session's reach, which the registry keeps and
 * answers for (`/runner-admit`, registry-runner.ts): in any reach its own
 * chats, its delegates' and their machines; with `devices` reach (the
 * default) every chat and device the person has, at a revocation epoch the
 * forward carries, so isolating the session closes it out of them
 * (runner-revocation.ts).
 *
 * Only what holds in every reach is cached (per isolate, ≤5 minutes; a
 * chat's host rarely changes): a reach the person can change is asked of
 * the registry on every request, so a change takes effect at once.
 * Refusals are never cached: a chat the machine created moments ago works
 * as soon as its row lands.
 */
import type { Verified } from "./auth";
import { AUTH_USER_HEADER, type Env } from "./env";
import { json } from "./http";

const OWN_TTL_MS = 5 * 60_000;
const MAX_CACHE = 10_000;
const own = new Map<string, number>();

/** A subagent's transcript doc is `{chatId}--sub--{toolUseId}` (nested ones
 * are named after the root chat too) and has no registry row of its own: it
 * belongs to whoever hosts the chat that spawned it. */
export const gatedChat = (chatId: string): string => chatId.split("--sub--", 1)[0] || chatId;

/** What the gate decided: a refusal to send back, or an admission and the
 * epoch it is revocable at (none: it holds in every reach). */
export type RunnerAdmission = { readonly refused: Response } | { readonly refused?: undefined; readonly epoch?: number };

const ADMITTED: RunnerAdmission = {};

const admit = async (
  env: Pick<Env, "REGISTRY_ROOMS">,
  auth: Verified,
  ask: { chat: string } | { target: string },
  refusal: () => Response,
  now: number
): Promise<RunnerAdmission> => {
  if (!auth.orgId || !auth.deviceId) return { refused: refusal() };
  const [param, value] = "chat" in ask ? ["chat", ask.chat] : ["target", ask.target];
  const key = `${auth.orgId}/${auth.userId}/${auth.deviceId}/${param}/${value}`;
  const until = own.get(key);
  if (until !== undefined && until > now) return ADMITTED;
  own.delete(key);

  const room = env.REGISTRY_ROOMS.get(env.REGISTRY_ROOMS.idFromName(`reg1/${auth.orgId}/${auth.userId}`));
  let answer: { ok?: unknown; epoch?: unknown };
  try {
    const query = `device=${encodeURIComponent(auth.deviceId)}&${param}=${encodeURIComponent(value)}`;
    const reply = await room.fetch(`https://registry/runner-admit?${query}`, {
      headers: { [AUTH_USER_HEADER]: auth.userId }
    });
    if (!reply.ok) throw new Error(`registry ${reply.status}`);
    answer = (await reply.json()) as typeof answer;
  } catch (e) {
    console.warn("runner gate: registry unavailable", String(e));
    return {
      refused: json(
        {
          error: "unavailable",
          message: "Couldn't check what this Cloud chat reaches; retry."
        },
        503
      )
    };
  }
  if (answer.ok !== true) return { refused: refusal() };
  if (typeof answer.epoch === "number") return { epoch: answer.epoch };
  if (own.size >= MAX_CACHE) own.clear();
  own.set(key, now + OWN_TTL_MS);
  return ADMITTED;
};

/** Gate a runner on chat `chatId`'s data. Other bearers pass. */
export const runnerChatGate = (
  env: Pick<Env, "REGISTRY_ROOMS">,
  auth: Verified,
  chatId: string,
  now = Date.now()
): Promise<RunnerAdmission> | RunnerAdmission => {
  if (auth.kind !== "runner") return ADMITTED;
  const refusal = () =>
    json(
      {
        error: "not_host",
        message: "This Cloud chat doesn't reach that chat."
      },
      403
    );
  return admit(env, auth, { chat: gatedChat(chatId) }, refusal, now);
};

/** Gate a runner on another device's room (a client link, a nudge). Its own
 * room, and read-only liveness, never reach here (runner-policy.ts). */
export const runnerDeviceGate = (
  env: Pick<Env, "REGISTRY_ROOMS">,
  auth: Verified,
  deviceId: string,
  now = Date.now()
): Promise<RunnerAdmission> | RunnerAdmission => {
  if (auth.kind !== "runner" || deviceId === auth.deviceId) return ADMITTED;
  const refusal = () =>
    json(
      {
        error: "forbidden",
        message: "This Cloud chat doesn't reach that device."
      },
      403
    );
  return admit(env, auth, { target: deviceId }, refusal, now);
};
