/** Small HTTP helpers shared by the Cloud/vault route modules. */

export const json = (value: unknown, status = 200): Response =>
  new Response(JSON.stringify(value), {
    status,
    headers: { "content-type": "application/json" }
  });

/** `{error, message}` — the error envelope of the Cloud/vault HTTP contract. */
export const jsonError = (status: number, error: string, message: string): Response =>
  json({ error, message }, status);

/**
 * Parse a JSON body of at most `maxBytes`; `undefined` for oversized or
 * malformed input. Several of these routes are reachable without a bearer
 * (`/runner/*`), so the size cap applies before any parsing.
 */
export const readJsonBody = async (request: Request, maxBytes: number): Promise<unknown> => {
  const declared = Number(request.headers.get("content-length") ?? "0");
  if (declared > maxBytes) return undefined;
  let text: string;
  try {
    text = await request.text();
  } catch {
    return undefined;
  }
  if (text.length > maxBytes) return undefined;
  try {
    return JSON.parse(text) as unknown;
  } catch {
    return undefined;
  }
};
