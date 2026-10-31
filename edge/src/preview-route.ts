import type { Verified } from "./auth";
import { AUTH_USER_HEADER, ROOM_KIND_HEADER, type Env } from "./env";
import { PREVIEW_PUBLISHER_HEADER, PREVIEW_SIGN_IN_HEADER } from "./preview-room";
/** Route identity comes exclusively from verified JWT claims. */
export function previewRoute(request: Request, env: Pick<Env, "PREVIEW_ROOMS">, auth: Verified): Promise<Response> | Response | undefined {
  const url = new URL(request.url);
  const parts = url.pathname.split("/").filter(Boolean);
  const id = /^[A-Za-z0-9_-]{1,128}$/;
  if (parts[0] !== "preview" || parts.length !== 3 || !id.test(parts[1]) || parts[2] !== "ws") return;
  if (auth.orgId !== parts[1]) return new Response("Forbidden", { status: 403 });
  // A Cloud machine joins only as itself, and only to publish: it never
  // sees the other devices' services nor opens a connection to one.
  const runner = auth.kind === "runner";
  const device = runner ? (auth.deviceId ?? "") : (url.searchParams.get("device") ?? "");
  if (!id.test(device)) return new Response("Invalid device", { status: 400 });
  // A device finishing a provider sign-in gets in past the device limit.
  const signIn = !runner && url.searchParams.get("signIn") === "1";
  const room = env.PREVIEW_ROOMS.get(env.PREVIEW_ROOMS.idFromName(`preview1/${parts[1]}/${auth.userId}`));
  url.pathname = "/ws"; url.search = `?device=${device}`;
  const headers = new Headers(request.headers);
  headers.set(AUTH_USER_HEADER, auth.userId); headers.delete(ROOM_KIND_HEADER);
  headers.delete(PREVIEW_PUBLISHER_HEADER); headers.delete(PREVIEW_SIGN_IN_HEADER);
  if (runner) headers.set(PREVIEW_PUBLISHER_HEADER, "1");
  if (signIn) headers.set(PREVIEW_SIGN_IN_HEADER, "1");
  return room.fetch(new Request(url, { method: request.method, headers }));
}
