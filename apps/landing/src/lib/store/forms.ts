// A form's post, the same way everywhere: signed in, the session's token
// echoed back (the edge checks it too, with the Origin, which Astro has
// already held to this site), the edge asked, the answer said once, and
// back to a page with 303 so a reload never posts again.
import type { AstroGlobal } from "astro";
import type { Result } from "./edge.ts";
import { setFlash, type FlashWord } from "./flash.ts";
import type { WebSession } from "./types.ts";
import { sitePath } from "./urls.ts";

/**
 * The most a form's body may be, in bytes. The longest field any form has
 * is 2,000 characters (a reviewer's words; 18 KB when every one is encoded
 * as three bytes thrice), so 64 KB is generous; past it the body is never
 * read whole, let alone parsed (the middleware answers 413 first when the
 * request says how long it is).
 */
export const MOST_FORM = 64 * 1024;

/** A request's body, read no further than `most` bytes: null past them. */
async function bodyOf(request: Request, most: number): Promise<Uint8Array<ArrayBuffer> | null> {
  const declared = Number(request.headers.get("content-length") ?? "0");
  if (!Number.isFinite(declared) || declared > most) return null;
  const reader = request.body?.getReader();
  if (!reader) return new Uint8Array();
  const chunks: Uint8Array[] = [];
  let total = 0;
  for (;;) {
    const { done, value } = await reader.read();
    if (done) break;
    total += value.byteLength;
    if (total > most) {
      await reader.cancel().catch(() => {});
      return null;
    }
    chunks.push(value);
  }
  const out = new Uint8Array(total);
  let at = 0;
  for (const chunk of chunks) {
    out.set(chunk, at);
    at += chunk.byteLength;
  }
  return out;
}

/** The post's fields, or null when it isn't a form (or is a body past MOST_FORM). */
export async function formOf(request: Request): Promise<FormData | null> {
  const type = request.headers.get("content-type") ?? "";
  if (!type.startsWith("application/x-www-form-urlencoded") && !type.startsWith("multipart/form-data")) return null;
  try {
    const body = await bodyOf(request, MOST_FORM);
    if (!body) return null;
    return await new Response(body, { headers: { "content-type": type } }).formData();
  } catch {
    return null;
  }
}

export const field = (form: FormData, name: string, max = 2000): string => {
  const value = form.get(name);
  return typeof value === "string" ? value.trim().slice(0, max) : "";
};

/**
 * Whether the post carries this session's token, compared in constant time
 * (every character looked at, whatever the first difference): the edge
 * checks it again, but the site is never the weaker copy.
 */
export const tokenMatches = (form: FormData, session: WebSession) => sameText(field(form, "csrf", 200), session.csrf);

/** Two strings compared without stopping at the first difference. */
export function sameText(a: string, b: string): boolean {
  const x = new TextEncoder().encode(a);
  const y = new TextEncoder().encode(b);
  let differ = x.length ^ y.length;
  for (let i = 0; i < Math.max(x.length, y.length); i++) differ |= (x[i] ?? 0) ^ (y[i] ?? 0);
  return differ === 0;
}

/** What to say about an answer from the edge. */
export function flashFor(result: Result<unknown>, done: FlashWord): FlashWord {
  if (result.ok) return done;
  switch (result.error) {
    case "conflict":
    case "not_found":
    case "unknown_token":
      return "conflict";
    case "invalid_state":
      return done === "reserved" ? "id_in_use" : "conflict";
    case "forbidden":
      return "not_allowed";
    case "signed_out":
    case "unauthenticated":
    case "bad_csrf":
      return "expired";
    case "bad_request":
      return "bad_input";
    case "rate_limited":
      return "slow_down";
    case "site_id":
      return "site_id";
    default:
      return "failed";
  }
}

/** Says `word` on the page at `to` (a path on this site), and goes there. */
export function done(astro: Pick<AstroGlobal, "cookies" | "redirect">, word: FlashWord, to: string): Response {
  setFlash(astro.cookies, word);
  return astro.redirect(sitePath(to), 303);
}

/**
 * Whether the browser says this visit came from another site (a link
 * elsewhere, or typed in). Browsers that don't say are taken at their word.
 */
export const fromElsewhere = (request: Request) => {
  const site = request.headers.get("sec-fetch-site");
  return site !== null && site !== "same-origin";
};
