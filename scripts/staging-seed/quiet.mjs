// What a seed prints names no account and shows no secret: an author is its
// slot. What's printed of another program's or a library's words (the CLI's,
// the engine's, the browser's, which quotes what it typed) passes through here.

/** How a program may write `secret`: as it is, in JSON, in a single-quoted
 * string, in an address or a form, in a page. */
const writings = (secret) => {
  const html = secret.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;");
  const form = new URLSearchParams({ s: secret }).toString().slice(2);
  return [
    secret,
    JSON.stringify(secret).slice(1, -1),
    secret.replace(/[\\']/g, "\\$&"),
    secret.replace(/'/g, "\\'"),
    encodeURIComponent(secret),
    form,
    form.replaceAll("+", "%20"),
    html,
    html.replace(/"/g, "&quot;"),
    html.replace(/"/g, "&quot;").replace(/'/g, "&#39;"),
    html.replace(/"/g, "&#34;").replace(/'/g, "&#39;"),
    html.replace(/"/g, "&quot;").replace(/'/g, "&#x27;"),
    html.replace(/"/g, "&quot;").replace(/'/g, "&apos;")
  ];
};

/** `text` without `secrets` (a login's values), a deploy token, a session's
 * token, an email address, or an @name. */
export const quiet = (text, secrets = []) => {
  let out = String(text);
  const written = secrets.filter((s) => typeof s === "string" && s.length > 2).flatMap(writings);
  for (const w of [...new Set(written)].sort((a, b) => b.length - a.length)) out = out.replace(new RegExp(w.replace(/[.*+?^${}()|[\]\\/]/g, "\\$&"), "gi"), "…");
  return out
    .replace(/zdt_[0-9A-Za-z]+/g, "zdt_…")
    .replace(/\beyJ[\w-]+\.[\w-]+\.[\w-]*/g, "…")
    .replace(/[^\s"'<>()]+@[^\s"'<>()]+\.[^\s"'<>()]+/g, "…")
    .replace(/(^|[\s"'(])@[a-z0-9][a-z0-9_-]*/gi, "$1@…");
};

/** A login's values, none of which is ever printed. */
export const secretsOf = (login) => Object.values(login ?? {}).filter((v) => typeof v === "string");

/** Keep a library from printing by itself: Playwright writes every call with
 * what it was given when these are set. Called before one is loaded. */
export const quietLibraries = () => {
  for (const name of ["DEBUG", "PWDEBUG", "NODE_DEBUG"]) delete process.env[name];
};
