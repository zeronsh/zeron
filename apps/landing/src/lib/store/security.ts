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

/** Each inline <style>'s text, as the browser hashes it. */
export function inlineStyles(html: string): string[] {
  const out: string[] = [];
  for (const match of html.matchAll(/<style(?:\s[^>]*)?>([^]*?)<\/style>/gi)) out.push(match[1]!);
  return out;
}

/** Whether the page carries any inline script (it must not). */
export function hasInlineScript(html: string): boolean {
  for (const match of html.matchAll(/<script\b([^>]*)>([^]*?)<\/script>/gi)) {
    const attributes = match[1]!;
    const json = /\btype\s*=\s*["']?application\/(ld\+)?json/i.test(attributes);
    if (!/\bsrc\s*=/.test(attributes) && !json && match[2]!.trim() !== "") return true;
  }
  return false;
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
