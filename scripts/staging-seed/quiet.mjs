// What a seed prints names no account and shows no secret: an author is its
// slot. What's printed of another program's or a library's words (the CLI's,
// the engine's, the browser's, which quotes what it typed) passes through here.

/** `text` without `secrets` (a login's values), a deploy token, a session's
 * token, an email address, or an @name. */
export const quiet = (text, secrets = []) => {
  let out = String(text);
  for (const secret of [...secrets].filter((s) => typeof s === "string" && s.length > 2).sort((a, b) => b.length - a.length)) {
    for (const written of new Set([secret, JSON.stringify(secret).slice(1, -1), encodeURIComponent(secret)])) out = out.replaceAll(written, "…");
  }
  return out
    .replace(/zdt_[0-9A-Za-z]+/g, "zdt_…")
    .replace(/\beyJ[\w-]+\.[\w-]+\.[\w-]*/g, "…")
    .replace(/[^\s"'<>()]+@[^\s"'<>()]+\.[^\s"'<>()]+/g, "…")
    .replace(/(^|[\s"'(])@[a-z0-9][a-z0-9_-]*/gi, "$1@…");
};

/** A login's values, none of which is ever printed. */
export const secretsOf = (login) => Object.values(login ?? {}).filter((v) => typeof v === "string");
