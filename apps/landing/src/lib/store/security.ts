// The headers on everything the Worker renders, and each page's Content
// Security Policy: scripts only from the site's own files (there is no
// inline script on these pages), styles from its files plus the exact
// inline <style> blocks the page carries, pictures and fonts from the site,
// nothing from anyone else, never framed.

export const SECURITY_HEADERS: Record<string, string> = {
  "x-content-type-options": "nosniff",
  "referrer-policy": "strict-origin-when-cross-origin",
  "x-frame-options": "DENY",
  "cross-origin-opener-policy": "same-origin",
  "permissions-policy": "camera=(), microphone=(), geolocation=(), payment=(), usb=()",
};

/** A raw-text element of a page (<style> or <script>): its attributes and its text. */
interface RawElement {
  tag: "style" | "script";
  attributes: string;
  text: string;
}

/**
 * The <style> and <script> elements of a page, found as a browser's
 * tokenizer finds them: a tag's attributes are read through their quotes,
 * and comments and text are passed over, so author text inside an
 * attribute ("<style>" in a description) is never taken for an element.
 */
export function rawElements(html: string): RawElement[] {
  const out: RawElement[] = [];
  let i = 0;
  const lower = html.toLowerCase();
  while (i < html.length) {
    const open = html.indexOf("<", i);
    if (open === -1) break;
    if (html.startsWith("<!--", open)) {
      const close = html.indexOf("-->", open + 4);
      i = close === -1 ? html.length : close + 3;
      continue;
    }
    const name = /^<\/?([a-zA-Z][a-zA-Z0-9-]*)/.exec(html.slice(open, open + 64));
    if (!name) {
      i = open + 1;
      continue;
    }
    // Through the tag's attributes, quotes and all, to its ">".
    let j = open + name[0].length;
    let quote: string | null = null;
    for (; j < html.length; j++) {
      const ch = html[j]!;
      if (quote) {
        if (ch === quote) quote = null;
      } else if (ch === '"' || ch === "'") quote = ch;
      else if (ch === ">") break;
    }
    const tag = name[1]!.toLowerCase();
    const closing = html[open + 1] === "/";
    i = j + 1;
    if (!closing && (tag === "style" || tag === "script")) {
      const end = lower.indexOf(`</${tag}`, i);
      const stop = end === -1 ? html.length : end;
      out.push({ tag, attributes: html.slice(open + name[0].length, j), text: html.slice(i, stop) });
      i = stop;
    }
  }
  return out;
}

/** Each inline <style>'s text, as the browser hashes it. */
export function inlineStyles(html: string): string[] {
  return rawElements(html).filter((e) => e.tag === "style").map((e) => e.text);
}

/** Whether the page carries any inline script (it must not; data blocks aren't scripts). */
export function hasInlineScript(html: string): boolean {
  return rawElements(html).some((e) => e.tag === "script" && !/\bsrc\s*=/i.test(e.attributes) && !/\btype\s*=\s*["']?application\/(ld\+)?json/i.test(e.attributes) && e.text.trim() !== "");
}

async function sha256(text: string): Promise<string> {
  const digest = await crypto.subtle.digest("SHA-256", new TextEncoder().encode(text));
  let binary = "";
  for (const byte of new Uint8Array(digest)) binary += String.fromCharCode(byte);
  return btoa(binary);
}

export async function contentSecurityPolicy(html: string): Promise<string> {
  const hashes = await Promise.all(inlineStyles(html).map(async (text) => `'sha256-${await sha256(text)}'`));
  return [
    "default-src 'none'",
    "script-src 'self'",
    `style-src 'self' ${[...new Set(hashes)].join(" ")}`.trim(),
    // (data: for the small icons the build inlines into the stylesheets.)
    "img-src 'self' data:",
    "font-src 'self'",
    "connect-src 'self'",
    "manifest-src 'self'",
    "form-action 'self'",
    "frame-ancestors 'none'",
    "base-uri 'none'",
    "object-src 'none'",
  ].join("; ");
}

/** What an HTML form can send without asking first (a cross-site one included). */
const FORM_TYPES = ["application/x-www-form-urlencoded", "multipart/form-data", "text/plain"];

/**
 * Whether `request` is a form posted from another site, as Astro's own
 * check decides it (security.checkOrigin, astro/dist/core/app/origin-check.js),
 * made here so the admin pages can answer it with their 404: Astro's runs
 * only where a page is, and so would say which pages are.
 */
export function isCrossSiteForm(request: Request): boolean {
  if (["GET", "HEAD", "OPTIONS"].includes(request.method)) return false;
  const type = request.headers.get("content-type");
  if (type && !FORM_TYPES.some((form) => type.toLowerCase().includes(form))) return false;
  switch (request.headers.get("sec-fetch-site")) {
    case null:
    case "":
      break;
    case "same-origin":
    case "none":
      return false;
    default:
      return true;
  }
  const origin = request.headers.get("origin");
  return origin !== null && origin !== new URL(request.url).origin;
}
