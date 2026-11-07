/**
 * Attachment bytes in transit to a Cloud machine: `/blob/{chatId}/attachments/{uploadId}`.
 *
 * A Cloud machine may be asleep when a user sends an image, and a peer link
 * can't reach a sleeping machine. So the sender stores the bytes here first,
 * then writes the message; the machine downloads them when it wakes and
 * deletes the object once its own copy is on disk.
 *
 * Who may touch an object: the key is under the caller's own user id, so only
 * that user's bearers can name it, and a Cloud machine is admitted only for a
 * chat it hosts (or a delegate it started; runner-access.ts).
 *
 * Retention: the machine's DELETE is the normal end. The bucket's lifecycle
 * rule on `attachments/` (30 days, see docs/design/cloud-attachments.md) catches
 * whatever a machine never collected. A message to a Cloud machine waits at most
 * 7 days before it expires, and a queued one for as long as its machine sleeps,
 * so an object outlives what can still want it. One that ages out anyway fails its
 * message with a retry, which uploads it again from the sender's copy.
 *
 * Quota: what a user has stored and not yet collected is capped, in bytes and in
 * files (`ATTACHMENT_QUOTA_*`). Collected, deleted and expired files free their
 * space. The check runs before each upload, so uploads racing each other can
 * pass it together.
 */
import type { Verified } from "./auth";
import type { Env } from "./env";
import { json } from "./http";
import { runnerChatGate } from "./runner-access";

/** The engine's own cap on an assembled upload (uploads.rs `MAX_BYTES`). */
export const MAX_ATTACHMENT_BYTES = 32 * 1024 * 1024;

const SHA256_RE = /^[0-9a-f]{64}$/;
/** The hex SHA-256 of the object, sent on PUT and returned on GET. */
export const SHA256_HEADER = "x-zeron-sha256";

export const attachmentKey = (userId: string, chatId: string, uploadId: string): string =>
  `attachments/${userId}/${chatId}/${uploadId}`;

export const DEFAULT_QUOTA_BYTES = 1024 * 1024 * 1024;
export const DEFAULT_QUOTA_OBJECTS = 500;

type QuotaEnv = Pick<Env, "ATTACHMENT_QUOTA_BYTES" | "ATTACHMENT_QUOTA_OBJECTS">;

const positive = (value: string | undefined, fallback: number): number => {
  const n = Number(value);
  return value !== undefined && Number.isSafeInteger(n) && n > 0 ? n : fallback;
};

export const quotaOf = (env: QuotaEnv) => ({
  bytes: positive(env.ATTACHMENT_QUOTA_BYTES, DEFAULT_QUOTA_BYTES),
  objects: positive(env.ATTACHMENT_QUOTA_OBJECTS, DEFAULT_QUOTA_OBJECTS)
});

/** What a user has stored, `except` the key about to be replaced. Stops counting
 * once past `stopAfter` files: the answer is "over" either way. */
const usageOf = async (
  bucket: R2Bucket,
  userId: string,
  except: string,
  stopAfter: number
): Promise<{ bytes: number; objects: number }> => {
  let bytes = 0;
  let objects = 0;
  let cursor: string | undefined;
  do {
    const page = await bucket.list({ prefix: `attachments/${userId}/`, cursor, limit: 1000 });
    for (const object of page.objects) {
      if (object.key === except) continue;
      bytes += object.size;
      objects += 1;
    }
    cursor = page.truncated ? page.cursor : undefined;
  } while (cursor && objects <= stopAfter);
  return { bytes, objects };
};

const hex = (bytes: ArrayBuffer): string =>
  [...new Uint8Array(bytes)].map((b) => b.toString(16).padStart(2, "0")).join("");

export const handleAttachmentBlob = async (
  request: Request,
  env: Pick<Env, "BLOBS" | "REGISTRY_ROOMS"> & QuotaEnv,
  auth: Verified,
  chatId: string,
  uploadId: string
): Promise<Response> => {
  const notHost = await runnerChatGate(env, auth, chatId);
  if (notHost) return notHost;
  const key = attachmentKey(auth.userId, chatId, uploadId);

  switch (request.method) {
    case "PUT": {
      // R2 needs the length up front to take a stream, and the header is the
      // only way to refuse an oversized file before storing any of it.
      const declared = request.headers.get("content-length");
      if (declared === null) return json({ error: "length_required" }, 411);
      const length = Number(declared);
      if (!Number.isSafeInteger(length) || length < 0) return json({ error: "bad_length" }, 400);
      if (length > MAX_ATTACHMENT_BYTES) return json({ error: "too_large" }, 413);
      const sha256 = request.headers.get(SHA256_HEADER)?.toLowerCase();
      if (sha256 !== undefined && !SHA256_RE.test(sha256)) return json({ error: "bad_checksum" }, 400);
      const quota = quotaOf(env);
      const used = await usageOf(env.BLOBS, auth.userId, key, quota.objects);
      if (used.objects + 1 > quota.objects || used.bytes + length > quota.bytes) {
        return json(
          {
            error: "quota_exceeded",
            message: "Too many files are waiting for Cloud machines to collect them.",
            usedBytes: used.bytes,
            usedObjects: used.objects,
            limitBytes: quota.bytes,
            limitObjects: quota.objects
          },
          507
        );
      }
      try {
        await env.BLOBS.put(key, request.body ?? new Uint8Array(), {
          ...(sha256 ? { sha256 } : {}),
          httpMetadata: { contentType: "application/octet-stream" }
        });
      } catch (e) {
        // A storage hiccup, or bytes that don't match their checksum: either
        // way nothing was stored and sending the file again is the remedy.
        console.warn("attachment store failed", String(e));
        return json({ error: "store_failed" }, 503);
      }
      return json({ ok: true, bytes: length });
    }
    case "GET":
    case "HEAD": {
      const object = request.method === "GET" ? await env.BLOBS.get(key) : await env.BLOBS.head(key);
      if (!object) return json({ error: "not_found" }, 404);
      const headers = new Headers({
        "content-type": "application/octet-stream",
        "content-length": String(object.size),
        // Someone's private file: nothing downstream keeps or interprets it.
        "cache-control": "private, no-store",
        "x-content-type-options": "nosniff"
      });
      const sum = object.checksums.sha256;
      if (sum) headers.set(SHA256_HEADER, hex(sum));
      const body = request.method === "GET" && "body" in object ? (object as R2ObjectBody).body : null;
      return new Response(body, { headers });
    }
    case "DELETE":
      await env.BLOBS.delete(key);
      return json({ ok: true });
    default:
      return json({ error: "method_not_allowed" }, 405);
  }
};
