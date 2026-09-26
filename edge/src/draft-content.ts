import type { Env } from "./env";
const json = (body: unknown, status = 200) => new Response(JSON.stringify(body), { status, headers: { "content-type": "application/json" } });

export function draftContentRoute(request: Request, env: Pick<Env, "BLOBS">, auth: { userId: string; orgId?: string }): Promise<Response> | undefined {
  const parts = new URL(request.url).pathname.split("/").filter(Boolean);
  if (parts[0] !== "draft-content" || parts.length !== 3 || !/^[a-zA-Z0-9_-]{1,256}$/.test(parts[1]) || !/^[a-zA-Z0-9-]{1,128}$/.test(parts[2])) return undefined;
  return (async () => {
      if (auth.orgId !== parts[1]) return json({ error: "forbidden" }, 403);
      const key = `drafts/${auth.userId}/${parts[1]}/${parts[2]}`;
      if (request.method === "PUT") {
        const limit = 32 * 1024 * 1024;
        if (Number(request.headers.get("content-length") ?? 0) > limit) return json({ error: "too_large" }, 413);
        const body = await request.arrayBuffer();
        if (body.byteLength > limit) return json({ error: "too_large" }, 413);
        const stored = await env.BLOBS.put(key, body, { onlyIf: { etagDoesNotMatch: "*" } });
        if (!stored) {
          const existing = await env.BLOBS.get(key);
          const old = existing ? new Uint8Array(await existing.arrayBuffer()) : undefined;
          const next = new Uint8Array(body);
          if (!old || old.length !== next.length || old.some((b, i) => b !== next[i])) return json({ error: "immutable_revision" }, 409);
        }
        return json({ ok: true });
      }
      if (request.method === "GET") {
        const object = await env.BLOBS.get(key);
        return object ? new Response(object.body, { headers: { "content-type": "application/octet-stream", "cache-control": "private, max-age=31536000, immutable" } }) : json({ error: "not_found" }, 404);
      }
    return json({ error: "method_not_allowed" }, 405);
  })();
}
