// A deploy token as the store makes one (contract a315eef5a; the engine's
// data file and vectors, deploy_token): `zdt_`, then the 32 random bytes as
// one big-endian number in base62, 43 digits, then a CRC-32 (zlib's) of the
// ASCII of the prefix and the body, in base62, 6 digits. Its short id is
// the body's first 8; all the store keeps is the SHA-256 of the whole.

export const TOKEN = { prefix: "zdt_", alphabet: "0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz", bodyLen: 43, checkLen: 6, shortLen: 8 } as const;

export const base62 = (n: bigint, len: number) => {
  let out = "";
  for (let rest = n; rest > 0n; rest /= 62n) out = TOKEN.alphabet[Number(rest % 62n)] + out;
  return out.padStart(len, "0");
};

const CRC_TABLE = Array.from({ length: 256 }, (_, n) => {
  let c = n;
  for (let k = 0; k < 8; k++) c = c & 1 ? 0xedb88320 ^ (c >>> 1) : c >>> 1;
  return c >>> 0;
});
export const crc32 = (text: string) => {
  let c = 0xffffffff;
  for (let i = 0; i < text.length; i++) c = CRC_TABLE[(c ^ text.charCodeAt(i)) & 0xff]! ^ (c >>> 8);
  return (c ^ 0xffffffff) >>> 0;
};

/** The token for these 32 random bytes, and its short id. */
export function makeToken(random: Uint8Array): { token: string; short: string } {
  const hex = [...random].map((b) => b.toString(16).padStart(2, "0")).join("");
  const body = base62(BigInt(`0x${hex}`), TOKEN.bodyLen);
  const check = base62(BigInt(crc32(TOKEN.prefix + body)), TOKEN.checkLen);
  return { token: TOKEN.prefix + body + check, short: body.slice(0, TOKEN.shortLen) };
}

/** What the store keeps of a token: the SHA-256 of its ASCII, in hex. */
export async function tokenHash(token: string): Promise<string> {
  const digest = await crypto.subtle.digest("SHA-256", new TextEncoder().encode(token));
  return [...new Uint8Array(digest)].map((b) => b.toString(16).padStart(2, "0")).join("");
}

/** A token as read from a header: the token and its short id, or why it isn't one. */
export function readToken(raw: string): { ok: true; token: string; short: string } | { ok: false; error: "not_a_token" | "damaged" } {
  const token = raw.trim();
  if (!token.startsWith(TOKEN.prefix) || !/^[0-9A-Za-z]*$/.test(token.slice(TOKEN.prefix.length))) return { ok: false, error: "not_a_token" };
  const rest = token.slice(TOKEN.prefix.length);
  if (rest.length !== TOKEN.bodyLen + TOKEN.checkLen) return { ok: false, error: "damaged" };
  const body = rest.slice(0, TOKEN.bodyLen);
  if (base62(BigInt(crc32(TOKEN.prefix + body)), TOKEN.checkLen) !== rest.slice(TOKEN.bodyLen)) return { ok: false, error: "damaged" };
  return { ok: true, token, short: body.slice(0, TOKEN.shortLen) };
}

/** A token's id, as the store names it apart from its secret: `tok_` and 16 base62 (88 random bits). */
export const tokenId = (random: Uint8Array) => `tok_${base62(random.slice(0, 11).reduce((n, b) => (n << 8n) | BigInt(b), 0n), 16)}`;
export const TOKEN_ID = /^tok_[0-9A-Za-z]{16}$/;
