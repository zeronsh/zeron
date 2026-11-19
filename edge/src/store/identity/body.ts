/**
 * How a store route reads a request's body: once, and capped. The store
 * reads bodies only through these:
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

/** The largest body a store publisher route reads (contract amendment 34):
 * 20 MiB, the store's package limit. Measured through `readBody`, a package
 * of this size peaks at about 72 MB in a Worker. */
export const MAX_PUBLISH_BODY_BYTES = 20 * 1024 * 1024;

const bodies = new WeakMap<Request, Promise<Uint8Array | "too_large">>();

const declaredTooLarge = (request: Request, maxBytes: number): boolean => {
  const header = request.headers.get("content-length");
  if (header === null) return false;
  const declared = Number(header);
  return !(Number.isSafeInteger(declared) && declared >= 0 && declared <= maxBytes);
};

const readCapped = async (request: Request, maxBytes: number): Promise<Uint8Array | "too_large"> => {
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
  return bytes;
};

/** The request's body, at most `maxBytes` (else `"too_large"`). Read from
 * the wire once, under the first call's cap; later calls answer the same. */
export const readBody = async (request: Request, maxBytes = MAX_PUBLISH_BODY_BYTES): Promise<Uint8Array | "too_large"> => {
  let read = bodies.get(request);
  if (!read) {
    read = readCapped(request, maxBytes);
    bodies.set(request, read);
  }
  const bytes = await read;
  return typeof bytes !== "string" && bytes.byteLength > maxBytes ? "too_large" : bytes;
};

export class BodyRefused extends Error {
  constructor(readonly reason: "too_large") {
    super("The request body is too large.");
  }
}

/**
 * The request's body as a stream of at most `maxBytes`, for the store to
 * write straight to R2. It errors with `BodyRefused` past the cap, so a
 * write fed from it fails rather than storing it. One reader per request:
 * not after `readBody`.
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
  const source = request.body ?? new ReadableStream<Uint8Array>({ start: (controller) => controller.close() });
  let length = 0;
  return source.pipeThrough(
    new TransformStream<Uint8Array, Uint8Array>({
      async transform(chunk, controller) {
        length += chunk.byteLength;
        if (length > maxBytes) throw new BodyRefused("too_large");
        controller.enqueue(chunk);
      }
    })
  );
};
