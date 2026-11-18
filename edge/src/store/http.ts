/** Answers of the store's routes: JSON, `{ error, message }` on failure
 * (contract "Errors"), never cached unless a route says so. */

export const NO_STORE = "no-store";

export const answer = (value: unknown, status = 200, cacheControl = NO_STORE, extra: Record<string, string> = {}): Response =>
  new Response(JSON.stringify(value), {
    status,
    headers: { "content-type": "application/json; charset=utf-8", "cache-control": cacheControl, ...extra }
  });

export const fail = (status: number, error: string, message: string, extra: Record<string, unknown> = {}): Response =>
  answer({ error, message, ...extra }, status);

export const notFound = (): Response => fail(404, "not_found", "There's no such plugin.");

export const rateLimited = (): Response =>
  new Response(JSON.stringify({ error: "rate_limited", message: "Too many requests. Try again in a minute." }), {
    status: 429,
    headers: { "content-type": "application/json; charset=utf-8", "cache-control": NO_STORE, "retry-after": "60" }
  });

/** A body `readBody` refused: past its cap, or (a deploy key's signed
 * request) not the body that was signed. */
export const refusedBody = (reason: "too_large" | "body_mismatch"): Response =>
  reason === "too_large"
    ? fail(413, "too_large", "The request body is too large.")
    : fail(401, "invalid_signature", "The body isn't the one that was signed.");

/** A JSON body that is an object, or undefined. */
export const objectBody = (bytes: Uint8Array | undefined): Record<string, unknown> | undefined => {
  if (!bytes) return undefined;
  try {
    const value: unknown = JSON.parse(new TextDecoder().decode(bytes));
    return value !== null && typeof value === "object" && !Array.isArray(value) ? (value as Record<string, unknown>) : undefined;
  } catch {
    return undefined;
  }
};
