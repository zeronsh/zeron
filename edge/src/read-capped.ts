/** A request body read no further than a cap: what a route that refuses
 * large bodies reads with, so a refused one costs neither side its bytes. */

/** A request's body, if it's at most `max` bytes (none: past it). A declared
 * length past it is refused before a byte is read; a body of no declared
 * length is read until it passes `max`, and no further. */
export const readCapped = async (request: Request, max: number): Promise<Uint8Array | undefined> => {
  const declared = request.headers.get("content-length");
  if (declared !== null && Number(declared) > max) {
    request.body?.cancel().catch(() => {});
    return undefined;
  }
  if (!request.body) return new Uint8Array(0);
  const reader = request.body.getReader();
  const parts: Uint8Array[] = [];
  let total = 0;
  for (;;) {
    const { done, value } = await reader.read();
    if (done) break;
    total += value.byteLength;
    if (total > max) {
      reader.cancel().catch(() => {});
      return undefined;
    }
    parts.push(value);
  }
  const out = new Uint8Array(total);
  let at = 0;
  for (const part of parts) {
    out.set(part, at);
    at += part.byteLength;
  }
  return out;
};
