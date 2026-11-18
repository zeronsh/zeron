// A form's post, the same way everywhere: signed in, the session's token
// echoed back (the edge checks it too, with the Origin, which Astro has
// already held to this site), the edge asked, the answer said once, and
// back to a page with 303 so a reload never posts again.
import type { AstroGlobal } from "astro";
import type { Result } from "./edge.ts";
import { setFlash, type FlashWord } from "./flash.ts";
import type { WebSession } from "./types.ts";
import { sitePath } from "./urls.ts";

/** The post's fields, or null when it isn't a form. */
export async function formOf(request: Request): Promise<FormData | null> {
  const type = request.headers.get("content-type") ?? "";
  if (!type.startsWith("application/x-www-form-urlencoded") && !type.startsWith("multipart/form-data")) return null;
  try {
    return await request.formData();
  } catch {
    return null;
  }
}

export const field = (form: FormData, name: string, max = 2000): string => {
  const value = form.get(name);
  return typeof value === "string" ? value.trim().slice(0, max) : "";
};

/** Whether the post carries this session's token. */
export const tokenMatches = (form: FormData, session: WebSession) => field(form, "csrf", 200) === session.csrf;

/** What to say about an answer from the edge. */
export function flashFor(result: Result<unknown>, done: FlashWord): FlashWord {
  if (result.ok) return done;
  switch (result.error) {
    case "conflict":
    case "not_found":
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
