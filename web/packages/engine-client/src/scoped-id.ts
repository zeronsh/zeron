import { RpcError } from "./rpc-error";

/**
 * Cross-engine identity scoping — the port of `engine_registry.rs`'s
 * `ScopedId` (`crates/ui/src/engine_registry.rs:15-62`). A scoped id is a
 * client-local wire wrapper tagging a raw engine id with the engine it
 * belongs to; it never reaches an engine's own storage and exists purely so
 * a merged, multi-engine view can dispatch "open this chat" back to the
 * correct engine.
 *
 * Encoding scheme, verbatim from the desktop: `"engine:v1:"` followed by
 * unpadded base64url of the JSON array `[engineKey, rawId]`. The web scopes
 * EVERY id uniformly (the desktop leaves its local engine's ids unscoped;
 * the web has no local engine — every web engine is remote), so an id with
 * no prefix parses as `{ engine: null }` and the routing layer resolves
 * that against the default (active) engine.
 */

export const SCOPED_ID_PREFIX = "engine:v1:";

/** A parsed scoped identity; `engine: null` means unscoped (default engine). */
export interface ScopedId {
  readonly engine: string | null;
  readonly rawId: string;
}

/** Whether an id carries the cross-engine scope prefix. */
export function isScopedId(id: string): boolean {
  return id.startsWith(SCOPED_ID_PREFIX);
}

/** Scope a raw id to an engine — always emits the prefixed form on web. */
export function encodeScopedId(engineKey: string, rawId: string): string {
  return `${SCOPED_ID_PREFIX}${base64UrlEncode(JSON.stringify([engineKey, rawId]))}`;
}

/**
 * Parse a scoped id back to `(engineKey, rawId)`. An id with no prefix is
 * unscoped — `{ engine: null, rawId }`, resolved by the caller against the
 * default engine. Malformed payloads throw `RpcError("failed", …)` exactly
 * as the desktop's `ScopedId::parse` returns `RpcError::Failed`.
 */
export function parseScopedId(id: string): ScopedId {
  const encoded = id.startsWith(SCOPED_ID_PREFIX) ? id.slice(SCOPED_ID_PREFIX.length) : null;
  if (encoded === null) {
    return { engine: null, rawId: id };
  }
  const decoded = base64UrlDecode(encoded);
  if (decoded === null) {
    throw new RpcError("failed", "Invalid engine-scoped identity");
  }
  let value: unknown;
  try {
    value = JSON.parse(decoded);
  } catch {
    throw new RpcError("failed", "Invalid engine-scoped identity");
  }
  if (!Array.isArray(value) || value.length !== 2 || typeof value[0] !== "string" || typeof value[1] !== "string") {
    throw new RpcError("failed", "Invalid engine-scoped identity");
  }
  const engine = value[0] as string;
  if (engine.length === 0) {
    throw new RpcError("failed", "Missing engine identity");
  }
  return { engine, rawId: value[1] as string };
}

/** base64url without padding, from UTF-8 bytes (URL_SAFE_NO_PAD's alphabet). */
function base64UrlEncode(text: string): string {
  const bytes = new TextEncoder().encode(text);
  let binary = "";
  for (const byte of bytes) {
    binary += String.fromCharCode(byte);
  }
  return btoa(binary).replace(/\+/g, "-").replace(/\//g, "_").replace(/=+$/u, "");
}

/** Decode base64url (padding optional) back to text, or null when malformed. */
function base64UrlDecode(encoded: string): string | null {
  if (encoded.length === 0) {
    return null;
  }
  const normalized = encoded.replace(/-/g, "+").replace(/_/g, "/");
  try {
    const binary = atob(normalized);
    const bytes = new Uint8Array(binary.length);
    for (let index = 0; index < binary.length; index += 1) {
      bytes[index] = binary.charCodeAt(index);
    }
    return new TextDecoder().decode(bytes);
  } catch {
    return null;
  }
}
