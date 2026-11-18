/**
 * How a store route reads a request's body: once, capped, and, for a deploy
 * key's signed request, held to the hash that was signed.
 *
 * A signed request's signature covers its body's hash, which travels beside
 * it (signed-request.ts), so the request is authenticated before a byte of
 * the body is read; `expectBodyHash` then binds that hash to the request,
 * and both readers here refuse a body that doesn't match it. The store reads
 * bodies only through these:
 *
 * - `readBody`: the bytes, kept per Request, so every later call answers the
 *   same ones. For JSON and small bodies.
 * - `bodyStream`: the bytes as they arrive, for writing a package straight
 *   to R2 without holding it: the stream errors (and the write fails) past
 *   the cap, or at its end when the hash doesn't match.
 *
 * Neither allocates from `Content-Length`: memory follows what actually
 * arrives.
 */

/** The largest body a store publisher route reads: a whole `.zeron-plugin`
 * file at its limits, with room for its base64. The store lane sets the
 * real cap; a buffered read of this size doesn't fit a Worker twice, so a
 * package goes through `bodyStream`. */
export const MAX_PUBLISH_BODY_BYTES = 72 * 1024 * 1024;

const bodies = new WeakMap<Request, Promise<Uint8Array | "too_large" | "body_mismatch">>();
const expected = new WeakMap<Request, string>();

/** The body of `request` must hash (SHA-256, hex) to `hex`: what its
 * signature covers. Set by signed-request.ts before anyone reads it. */
export const expectBodyHash = (request: Request, hex: string): void => {
  expected.set(request, hex);
};

const hexOf = (digest: ArrayBuffer): string => [...new Uint8Array(digest)].map((b) => b.toString(16).padStart(2, "0")).join("");

const declaredTooLarge = (request: Request, maxBytes: number): boolean => {
  const header = request.headers.get("content-length");
  if (header === null) return false;
  const declared = Number(header);
  return !(Number.isSafeInteger(declared) && declared >= 0 && declared <= maxBytes);
};

const readCapped = async (request: Request, maxBytes: number): Promise<Uint8Array | "too_large" | "body_mismatch"> => {
  if (declaredTooLarge(request, maxBytes)) return "too_large";
  let bytes: Uint8Array = new Uint8Array(0);
  if (request.body) {
    const chunks: Uint8Array[] = [];
    let length = 0;
    const reader = request.body.getReader();
    for (;;) {
      const { done, value } = await reader.read();
      if (done) break;
      length += value.byteLength;
      if (length > maxBytes) {
        await reader.cancel().catch(() => undefined);
        return "too_large";
      }
      chunks.push(value);
    }
    if (chunks.length === 1) bytes = chunks[0]!;
    else {
      bytes = new Uint8Array(length);
      let at = 0;
      for (const chunk of chunks) {
        bytes.set(chunk, at);
        at += chunk.byteLength;
      }
    }
  }
  const hash = expected.get(request);
  if (hash !== undefined && hexOf(await crypto.subtle.digest("SHA-256", bytes)) !== hash) return "body_mismatch";
  return bytes;
};

/** The request's body, at most `maxBytes` (else `"too_large"`), and for a
 * signed request the body that was signed (else `"body_mismatch"`). Read
 * from the wire once, under the first call's cap; later calls answer the
 * same. */
export const readBody = async (
  request: Request,
  maxBytes = MAX_PUBLISH_BODY_BYTES
): Promise<Uint8Array | "too_large" | "body_mismatch"> => {
  let read = bodies.get(request);
  if (!read) {
    read = readCapped(request, maxBytes);
    bodies.set(request, read);
  }
  const bytes = await read;
  return typeof bytes !== "string" && bytes.byteLength > maxBytes ? "too_large" : bytes;
};

export class BodyRefused extends Error {
  constructor(readonly reason: "too_large" | "body_mismatch") {
    super(reason === "too_large" ? "The request body is too large." : "The body isn't the one that was signed.");
  }
}

/**
 * The request's body as a stream of at most `maxBytes`, for the store to
 * write straight to R2. It errors with `BodyRefused` past the cap, or, for
 * a signed request, at its end when the body isn't the one signed, so a write
 * fed from it fails rather than storing it. One reader per request: not
 * after `readBody`. Workers only (`crypto.DigestStream`).
 */
export const bodyStream = (request: Request, maxBytes = MAX_PUBLISH_BODY_BYTES): ReadableStream<Uint8Array> => {
  if (bodies.has(request)) throw new Error("bodyStream: this request's body was already read");
  bodies.set(request, Promise.resolve("too_large"));
  if (declaredTooLarge(request, maxBytes)) {
    return new ReadableStream({
      start(controller) {
        controller.error(new BodyRefused("too_large"));
      }
    });
  }
  // No body is an empty one, held to the signed hash all the same.
  const source = request.body ?? new ReadableStream<Uint8Array>({ start: (controller) => controller.close() });
  const hash = expected.get(request);
  const digest = hash === undefined ? undefined : new crypto.DigestStream("SHA-256");
  const digestWriter = digest?.getWriter();
  let length = 0;
  return source.pipeThrough(
    new TransformStream<Uint8Array, Uint8Array>({
      async transform(chunk, controller) {
        length += chunk.byteLength;
        if (length > maxBytes) throw new BodyRefused("too_large");
        await digestWriter?.write(chunk);
        controller.enqueue(chunk);
      },
      async flush() {
        if (!digest || !digestWriter) return;
        await digestWriter.close();
        if (hexOf(await digest.digest) !== hash) throw new BodyRefused("body_mismatch");
      }
    })
  );
};
