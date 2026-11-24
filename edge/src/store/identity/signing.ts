/**
 * Small shared pieces of the store's identity routes: a plugin id's shape,
 * and SHA-256 as hex.
 */

/** A store plugin id (manifest.ts's rule, as the identity routes check it). */
export const PLUGIN_ID_RE = /^[a-z0-9][a-z0-9-]{1,39}$/;

/** SHA-256 of `bytes` (or a string's UTF-8), lowercase hex. */
export const sha256Hex = async (bytes: Uint8Array | string): Promise<string> => {
  const data = typeof bytes === "string" ? new TextEncoder().encode(bytes) : bytes;
  const digest = await crypto.subtle.digest("SHA-256", data);
  return [...new Uint8Array(digest)].map((b) => b.toString(16).padStart(2, "0")).join("");
};
