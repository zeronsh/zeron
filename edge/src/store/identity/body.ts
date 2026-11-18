/**
 * One read of a request's body, shared by everyone who needs it. Checking a
 * deploy key's signature (signed-request.ts) has to hash the body, and the
 * publish route then needs the same bytes: a package can be tens of MB, and a
 * Worker can't hold it twice. So the body is read once, capped, and kept per
 * Request; every later `readBody` answers the same bytes. Store routes read
 * bodies only through this.
 */

/** The largest body a store publisher route reads: a whole `.zeron-plugin`
 * file at its limits, with room for its base64. */
export const MAX_PUBLISH_BODY_BYTES = 72 * 1024 * 1024;

const bodies = new WeakMap<Request, Promise<Uint8Array | "too_large">>();

/**
 * The body, at most `maxBytes`. With a declared length (what any uploader
 * sends) it fills one buffer of that size, so a package is held once; a body
 * longer or shorter than it declared is refused. Without one, the chunks are
 * joined at the end.
 */
const readCapped = async (request: Request, maxBytes: number): Promise<Uint8Array | "too_large"> => {
  const header = request.headers.get("content-length");
  const declared = header === null ? undefined : Number(header);
  if (declared !== undefined && !(Number.isSafeInteger(declared) && declared >= 0)) return "too_large";
  if (declared !== undefined && declared > maxBytes) return "too_large";
  if (!request.body) return new Uint8Array(0);
  const reader = request.body.getReader();
  const refuse = async (): Promise<"too_large"> => {
    await reader.cancel().catch(() => undefined);
    return "too_large";
  };
  if (declared !== undefined) {
    const bytes = new Uint8Array(declared);
    let at = 0;
    for (;;) {
      const { done, value } = await reader.read();
      if (done) break;
      if (at + value.byteLength > declared) return refuse();
      bytes.set(value, at);
      at += value.byteLength;
    }
    return at === declared ? bytes : "too_large";
  }
  const chunks: Uint8Array[] = [];
  let length = 0;
  for (;;) {
    const { done, value } = await reader.read();
    if (done) break;
    length += value.byteLength;
    if (length > maxBytes) return refuse();
    chunks.push(value);
  }
  const bytes = new Uint8Array(length);
  let at = 0;
  for (const chunk of chunks) {
    bytes.set(chunk, at);
    at += chunk.byteLength;
  }
  return bytes;
};

/** The request's body, at most `maxBytes` (else `"too_large"`). Read from
 * the wire once, under the first call's cap; later calls answer the same
 * bytes (or `"too_large"`, past their own cap or the first's). */
export const readBody = async (request: Request, maxBytes = MAX_PUBLISH_BODY_BYTES): Promise<Uint8Array | "too_large"> => {
  let read = bodies.get(request);
  if (!read) {
    read = readCapped(request, maxBytes);
    bodies.set(request, read);
  }
  const bytes = await read;
  return bytes === "too_large" || bytes.byteLength > maxBytes ? "too_large" : bytes;
};
