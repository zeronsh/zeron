// The site's security headers and its Content Security Policy, in one place
// for the Worker (security.ts) and for the build (astro.config.mjs writes
// them into the built files' _headers for the 404 page, which the Worker
// never renders). Plain JavaScript so the build's config can read it.
// @ts-check

/** @type {Record<string, string>} */
export const SECURITY_HEADERS = {
  "x-content-type-options": "nosniff",
  "referrer-policy": "strict-origin-when-cross-origin",
  "x-frame-options": "DENY",
  "cross-origin-opener-policy": "same-origin",
  "permissions-policy": "camera=(), microphone=(), geolocation=(), payment=(), usb=()",
};

/**
 * The policy, given the build's own inline styles' hashes (inline-styles.json).
 * @param {readonly string[]} hashes
 */
export const policyFor = (hashes) =>
  [
    "default-src 'none'",
    "script-src 'self'",
    `style-src 'self' ${hashes.map((h) => `'sha256-${h}'`).join(" ")}`.trim(),
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
