import { AUTH_USER_HEADER, ROOM_KIND_HEADER, RUNNER_ACCOUNT_HEADER, RUNNER_DEVICE_HEADER, type Env } from "./env";

/** Canonical chat identity comes from the authenticated route, never a
 * sender-supplied query. Also used by the workerd routing fixture. */
export function chatRoute(request: Request, env: Pick<Env, "CHAT_ROOMS">, userId: string): Promise<Response> | Response | undefined {
  const url = new URL(request.url);
  const parts = url.pathname.split("/").filter(Boolean);
  const id = /^[A-Za-z0-9_-]{1,128}$/;
  if (parts[0] !== "chat2" || !parts[1] || !id.test(parts[1]) || !parts[2]) return;
  const chat = parts[1];
  const route = parts[2];
  if (parts.length !== 3) return new Response("not found", { status: 404 });
  if (route === "ws") {
    if (request.headers.get("upgrade")?.toLowerCase() !== "websocket") {
      return new Response("expected websocket", { status: 426 });
    }
    const device = url.searchParams.get("device") ?? "";
    const host = url.searchParams.get("hostDevice");
    url.search = "";
    if (id.test(device)) url.searchParams.set("device", device);
    if (host !== null) url.searchParams.set("hostDevice", host);
  } else {
    const routes: Record<string, string[]> = {
      checkpoint: ["GET", "POST"], rows: ["GET", "POST"],
      tail: ["GET", "PUT"], diff: ["GET", "PUT"], stats: ["GET"], reset: ["POST"]
    };
    if (!routes[route]?.includes(request.method)) return new Response("not found", { status: 404 });
  }
  url.searchParams.set("chatId", chat);
  url.pathname = `/${route}`;
  const headers = new Headers(request.headers);
  headers.delete(ROOM_KIND_HEADER);
  // Worker-controlled runner identity: never passed through from a caller.
  headers.delete(RUNNER_DEVICE_HEADER);
  headers.delete(RUNNER_ACCOUNT_HEADER);
  headers.set(AUTH_USER_HEADER, userId);
  const room = env.CHAT_ROOMS.get(env.CHAT_ROOMS.idFromName(`chat2/${chat}`));
  return room.fetch(new Request(url, { method: request.method, body: request.body, headers }));
}
