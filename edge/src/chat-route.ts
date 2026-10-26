import { AUTH_USER_HEADER, ROOM_KIND_HEADER, RUNNER_ACCOUNT_HEADER, RUNNER_DEVICE_HEADER, type Env } from "./env";

/** Room generations this Worker serves: chat2 (docs/chat2-sync.md) and
 * chat3, which is the chat2 log plus the transcript plane
 * (docs/chat3-sync.md). Both live in the CHAT_ROOMS namespace, named
 * `{gen}/{chatId}`, so the same chat id never shares a room across gens. */
const ROUTES: Record<string, Record<string, string[]>> = {
  chat2: {
    checkpoint: ["GET", "POST"], rows: ["GET", "POST"],
    tail: ["GET", "PUT"], diff: ["GET", "PUT"], stats: ["GET"], reset: ["POST"]
  },
  chat3: {
    checkpoint: ["GET", "POST"], rows: ["GET", "POST"], items: ["GET", "POST"],
    stats: ["GET"], reset: ["POST"]
  }
};
const PART = /^[A-Za-z0-9_.:#~-]{1,200}$/;

/** Canonical chat identity comes from the authenticated route, never a
 * sender-supplied query. Also used by the workerd routing fixture. */
export function chatRoute(request: Request, env: Pick<Env, "CHAT_ROOMS">, userId: string): Promise<Response> | Response | undefined {
  const url = new URL(request.url);
  const parts = url.pathname.split("/").filter(Boolean);
  const id = /^[A-Za-z0-9_-]{1,128}$/;
  const gen = parts[0] ?? "";
  const routes = ROUTES[gen];
  if (!routes || !parts[1] || !id.test(parts[1]) || !parts[2]) return;
  const chat = parts[1];
  const route = parts[2];
  // chat3 tool outputs: `/chat3/{chat}/outputs/{partId}` (one level deeper).
  const output = gen === "chat3" && route === "outputs" && parts.length === 4;
  if (parts.length !== 3 && !output) return new Response("not found", { status: 404 });
  if (output) {
    let part: string;
    try {
      part = decodeURIComponent(parts[3]!);
    } catch {
      return new Response("not found", { status: 404 });
    }
    if (!PART.test(part) || (request.method !== "GET" && request.method !== "PUT")) {
      return new Response("not found", { status: 404 });
    }
  } else if (route === "ws") {
    if (request.headers.get("upgrade")?.toLowerCase() !== "websocket") {
      return new Response("expected websocket", { status: 426 });
    }
    const device = url.searchParams.get("device") ?? "";
    const host = url.searchParams.get("hostDevice");
    url.search = "";
    if (id.test(device)) url.searchParams.set("device", device);
    if (host !== null) url.searchParams.set("hostDevice", host);
  } else {
    if (!routes[route]?.includes(request.method)) return new Response("not found", { status: 404 });
  }
  url.searchParams.set("chatId", chat);
  url.pathname = output ? `/outputs/${parts[3]}` : `/${route}`;
  const headers = new Headers(request.headers);
  headers.delete(ROOM_KIND_HEADER);
  // Worker-controlled runner identity: never passed through from a caller.
  headers.delete(RUNNER_DEVICE_HEADER);
  headers.delete(RUNNER_ACCOUNT_HEADER);
  headers.set(AUTH_USER_HEADER, userId);
  const room = env.CHAT_ROOMS.get(env.CHAT_ROOMS.idFromName(`${gen}/${chat}`));
  return room.fetch(new Request(url, { method: request.method, body: request.body, headers }));
}
