/**
 * A deploy token (contract a315eef5a, "Deploy tokens replace deploy keys"):
 * a plain bearer secret that publishes versions of an account's plugins
 * from CI, as npm, PyPI and crates.io tokens do.
 *
 *   zdt_ + body (43) + checksum (6)        53 characters, ^zdt_[0-9A-Za-z]{49}$
 *
 * - body: 32 bytes from the edge's CSPRNG as one big-endian integer in
 *   base62 (`0-9A-Za-z`), left-padded with "0" to 43 characters;
 * - checksum: CRC-32 (IEEE, zlib's) of the ASCII `zdt_` + body, in the same
 *   base62, left-padded to 6: a typo fails before any lookup, and leak
 *   scanners can recognise one;
 * - short: the body's first 8 characters, the token's public name in lists,
 *   notices and the audit log.
 *
 * The edge returns a token once, when it is made, and keeps only its
 * SHA-256 (hex) and its short id. A token is never logged, never put in an
 * error or an audit row.
 */
import { constantTimeEqual } from "../../cloud/crypto";

export const TOKEN_PREFIX = "zdt_";
const ALPHABET = "0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz";
const BODY_LENGTH = 43;
const CHECKSUM_LENGTH = 6;
const SHORT_LENGTH = 8;
const TOKEN_RE = /^zdt_[0-9A-Za-z]{49}$/;

/** `n` in base62, left-padded with "0" to `width`. */
const base62 = (n: bigint, width: number): string => {
  let out = "";
  for (let rest = n; rest > 0n; rest /= 62n) out = ALPHABET[Number(rest % 62n)]! + out;
  return out.padStart(width, "0");
};

const CRC_TABLE = (() => {
  const table = new Uint32Array(256);
  for (let n = 0; n < 256; n++) {
    let c = n;
    for (let k = 0; k < 8; k++) c = c & 1 ? 0xedb88320 ^ (c >>> 1) : c >>> 1;
    table[n] = c >>> 0;
  }
  return table;
})();

/** CRC-32 (IEEE, as zlib's `crc32`) of `bytes`. */
export const crc32 = (bytes: Uint8Array): number => {
  let crc = 0xffffffff;
  for (const b of bytes) crc = CRC_TABLE[(crc ^ b) & 0xff]! ^ (crc >>> 8);
  return (crc ^ 0xffffffff) >>> 0;
};

/** The checksum of `zdt_` + `body`. */
export const tokenChecksum = (body: string): string => base62(BigInt(crc32(new TextEncoder().encode(TOKEN_PREFIX + body))), CHECKSUM_LENGTH);

/** The token with `body` (its 43 characters). */
export const tokenFromBody = (body: string): string => `${TOKEN_PREFIX}${body}${tokenChecksum(body)}`;

/** A new token from 32 random bytes. */
export const newDeployToken = (): string => {
  const bytes = crypto.getRandomValues(new Uint8Array(32));
  const n = bytes.reduce((acc, b) => (acc << 8n) | BigInt(b), 0n);
  return tokenFromBody(base62(n, BODY_LENGTH));
};

/** `text` read as a token: its short id when it is one (the prefix, the
 * length, the alphabet, and the checksum all right), else undefined. */
export const parseDeployToken = (text: string): { readonly short: string } | undefined => {
  if (!TOKEN_RE.test(text)) return undefined;
  const body = text.slice(TOKEN_PREFIX.length, TOKEN_PREFIX.length + BODY_LENGTH);
  const checksum = text.slice(TOKEN_PREFIX.length + BODY_LENGTH);
  if (!constantTimeEqual(checksum, tokenChecksum(body))) return undefined;
  return { short: body.slice(0, SHORT_LENGTH) };
};

/** Whether `text` looks like a deploy token (the prefix alone): such a bearer
 * is never a session's, and is answered as a token wherever it arrives. */
export const looksLikeDeployToken = (text: string): boolean => text.startsWith(TOKEN_PREFIX);

/** A token's stored form: SHA-256, hex. */
export const deployTokenHash = async (token: string): Promise<string> => {
  const digest = await crypto.subtle.digest("SHA-256", new TextEncoder().encode(token));
  return [...new Uint8Array(digest)].map((b) => b.toString(16).padStart(2, "0")).join("");
};

/** A token's own id (for revoking it; never derived from its secret). */
export const newDeployTokenId = (): string => {
  // (88 random bits fit 16 base62 characters: 2^88 < 62^16.)
  const bytes = crypto.getRandomValues(new Uint8Array(11));
  return `tok_${base62(bytes.reduce((acc, b) => (acc << 8n) | BigInt(b), 0n), 16)}`;
};
export const DEPLOY_TOKEN_ID_RE = /^tok_[0-9A-Za-z]{16}$/;
