import type { Verified } from "./auth";
import { AUTH_USER_HEADER, ROOM_KIND_HEADER, type Env } from "./env";
import {
  DRAFT_ID_RE,
  DRAFT_ROUTES,
  MAX_CHECKPOINT_BYTES,
  draftRoomName,
  parseDraftPath
} from "./draft-protocol";

const json = (value: unknown, status: number): Response =>
  new Response(JSON.stringify(value), {
    status,
    headers: { "content-type": "application/json" }
  });

/**
 * `/draft/:orgId/:chatId/{ws,checkpoint,rows,epoch,discard,stats}` — the
 * per-user, per-chat composer-draft rooms (docs/draft-sync.md §1). Identity
 * comes exclusively from the verified auth claims: the org must match the URL
 * and the room name embeds the caller's OWN user id, so nobody can address
 * another user's draft and there is no ownership-claim race. The DO trusts the
 * AUTH_USER_HEADER stamped here and nothing else.
 *
 * Returns `undefined` for paths that are not draft routes.
 */
export async function draftRoute(
  request: Request,
  env: Pick<Env, "DRAFT_ROOMS">,
  auth: Verified
): Promise<Response | undefined> {
  const url = new URL(request.url);
  const path = parseDraftPath(url.pathname);
  if (path.kind === "notDraft") return undefined;
  if (path.kind === "notFound") return json({ error: "not found" }, 404);
  if (auth.orgId !== path.orgId) return json({ error: "forbidden" }, 403);
  if (!DRAFT_ID_RE.test(path.chatId)) return json({ error: "bad chat id" }, 400);

  let search = url.search;
  if (path.action === "ws") {
    if (request.headers.get("upgrade")?.toLowerCase() !== "websocket") {
      return json({ error: "expected websocket" }, 426);
    }
    // Only epoch + a validated device id reach the DO (log-injection hygiene,
    // same rationale as chat2's deviceParam).
    const params = new URLSearchParams();
    const epoch = url.searchParams.get("epoch");
    if (epoch !== null) params.set("epoch", epoch);
    const device = url.searchParams.get("device") ?? "";
    if (DRAFT_ID_RE.test(device)) params.set("device", device);
    search = `?${params.toString()}`;
  } else if (
    !Object.hasOwn(DRAFT_ROUTES, path.action) ||
    !DRAFT_ROUTES[path.action]?.includes(request.method)
  ) {
    // `hasOwn`: an action like "constructor" must 404, not resolve to an Object.prototype member.
    return json({ error: "not found" }, 404);
  }

  const ns = env.DRAFT_ROOMS;
  const stub = ns.get(ns.idFromName(draftRoomName(path.orgId, auth.userId, path.chatId)));
  const target = new URL(request.url);
  target.pathname = `/${path.action}`;
  target.search = search;
  const headers = new Headers(request.headers);
  headers.delete(ROOM_KIND_HEADER);
  headers.set(AUTH_USER_HEADER, auth.userId);
  // Draft bodies are tiny (row <= 64 KiB, checkpoint <= 256 KiB): buffer so
  // the DO can reject early (409/413) without erroring a half-read stream.
  if (request.method === "POST") {
    // Refuse oversized bodies before buffering them (the DO enforces the exact per-route caps).
    const declared = Number(request.headers.get("content-length") ?? "0");
    if (Number.isFinite(declared) && declared > MAX_CHECKPOINT_BYTES + 1024) {
      return json({ error: "too_large" }, 413);
    }
  }
  const body = request.method === "POST" ? await request.arrayBuffer() : undefined;
  return stub.fetch(new Request(target.toString(), { method: request.method, headers, body }));
}
