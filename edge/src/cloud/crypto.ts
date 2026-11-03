/**
 * WebCrypto helpers for the Cloud device's runner identity
 * (docs/design/cloud-device.md, "Runner identity"). Pure WebCrypto, so the
 * same code runs in workerd and in the Node unit tier.
 */

const B64URL_RE = /^[A-Za-z0-9_-]*$/;

export const b64urlEncode = (bytes: Uint8Array): string => {
  let binary = "";
  for (const byte of bytes) binary += String.fromCharCode(byte);
  return btoa(binary).replace(/\+/g, "-").replace(/\//g, "_").replace(/=+$/, "");
};

/** Strict: base64url alphabet only, no padding. `undefined` on anything else. */
export const b64urlDecode = (text: string): Uint8Array | undefined => {
  if (!B64URL_RE.test(text) || text.length % 4 === 1) return undefined;
  try {
    const binary = atob(text.replace(/-/g, "+").replace(/_/g, "/"));
    return Uint8Array.from(binary, (c) => c.charCodeAt(0));
  } catch {
    return undefined;
  }
};

/** 32 random bytes, base64url: the one-time enrollment code. */
export const randomCode = (): string => b64urlEncode(crypto.getRandomValues(new Uint8Array(32)));

export const sha256B64url = async (text: string): Promise<string> =>
  b64urlEncode(new Uint8Array(await crypto.subtle.digest("SHA-256", new TextEncoder().encode(text))));

/** Length-independent-of-content comparison for equal-length digests. */
export const constantTimeEqual = (a: string, b: string): boolean => {
  if (a.length !== b.length) return false;
  let diff = 0;
  for (let i = 0; i < a.length; i++) diff |= a.charCodeAt(i) ^ b.charCodeAt(i);
  return diff === 0;
};

/** A raw Ed25519 public key is exactly 32 bytes. */
export const parseEd25519PublicKey = (publicKey: string): Uint8Array | undefined => {
  const raw = b64urlDecode(publicKey);
  return raw?.length === 32 ? raw : undefined;
};

export const runnerTokenMessage = (
  orgId: string,
  userId: string,
  deviceId: string,
  ts: number
): string => `zeron-runner-token\n${orgId}\n${userId}\n${deviceId}\n${ts}`;

/** Verify a base64url Ed25519 signature over `message` (UTF-8). Any malformed
 * input is simply "not verified" — callers never need to distinguish. */
export const verifyEd25519 = async (
  publicKey: string,
  message: string,
  signature: string
): Promise<boolean> => {
  const raw = parseEd25519PublicKey(publicKey);
  const sig = b64urlDecode(signature);
  if (!raw || sig?.length !== 64) return false;
  try {
    const key = await crypto.subtle.importKey("raw", raw, { name: "Ed25519" }, false, ["verify"]);
    return await crypto.subtle.verify({ name: "Ed25519" }, key, sig, new TextEncoder().encode(message));
  } catch {
    return false;
  }
};
