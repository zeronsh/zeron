/**
 * Record encryption. Every credential is AES-256-GCM sealed under a KEK held
 * as a vault Worker SECRET (`VAULT_KEK`), never in Durable Object storage: a
 * storage dump alone decrypts nothing. (The trade-off, accepted to avoid an
 * external KMS: secrets and storage both live in the Cloudflare account, so an
 * account compromise could reach both.)
 *
 * Envelope: `v2.{kid}.{base64url iv}.{base64url ciphertext‖tag}`, a fresh
 * random 96-bit IV per seal, AAD = `{userId}|{provider}|{generation}`. The AAD
 * binds a ciphertext to its slot and generation, so a row copied into another
 * user's DO — or an old generation replayed over a new one — fails to open.
 *
 * Rotation: `kid` = first 16 hex chars of SHA-256(raw key), so an envelope
 * names the key that sealed it. Seals always use `VAULT_KEK`; opens accept
 * `VAULT_KEK` or `VAULT_KEK_PREVIOUS` and report when the previous key was
 * used, so callers re-seal under the current key (VaultAccount does, lazily,
 * on access). An envelope whose kid matches neither surfaces as `unavailable`
 * naming the kid: "sealed under a key that is no longer configured".
 */
import { fromBase64, fromBase64Url, fromUtf8, toBase64Url, toHex, utf8 } from "./encoding";
import type { Env } from "./env";

const ENVELOPE_VERSION = "v2";
const KID_RE = /^[0-9a-f]{16}$/;

/** The KEK is missing, malformed, or no longer configured. Surfaces as
 * `unavailable` (503); never cached, so fixing the secret recovers at once. */
export class VaultUnavailable extends Error {}

/** An envelope sealed under a key that is neither `VAULT_KEK` nor
 * `VAULT_KEK_PREVIOUS`. The operator signal after a premature key removal. */
export class UnknownKeyError extends VaultUnavailable {
  constructor(readonly kid: string) {
    super(`record sealed under key ${kid}, which is no longer configured`);
  }
}

export interface Kek {
  readonly kid: string;
  /** Non-extractable AES-GCM key. */
  readonly key: CryptoKey;
}

export interface Keyring {
  /** Seals everything; opens its own envelopes. */
  readonly current: Kek;
  /** Opens envelopes from before the last rotation, until they are re-sealed. */
  readonly previous?: Kek;
}

export interface Opened {
  readonly plaintext: string;
  readonly kid: string;
  /** Sealed under the previous key: re-seal under the current one. */
  readonly rewrap: boolean;
}

export const recordAad = (userId: string, provider: string, generation: number): string =>
  `${userId}|${provider}|${generation}`;

/** First 16 hex chars of SHA-256(raw key bytes). Identifies, never reveals. */
export const keyId = async (raw: Uint8Array<ArrayBuffer>): Promise<string> =>
  toHex(await crypto.subtle.digest("SHA-256", raw)).slice(0, 16);

/** Imported keys per isolate, by kid. No TTL: the secret is the source of
 * truth, and a changed secret has a different kid. Only RESOLVED keys are
 * cached (module memory is shared by every DO in the isolate, and workerd
 * does not let one object's request await another's pending work). */
const imported = new Map<string, CryptoKey>();

/** Test hook: forget every imported key. */
export const clearKeyCache = (): void => imported.clear();

const loadKek = async (name: string, secret: string): Promise<Kek> => {
  let raw: Uint8Array<ArrayBuffer>;
  try {
    raw = fromBase64(secret);
  } catch {
    throw new VaultUnavailable(`${name} is not valid base64`);
  }
  try {
    if (raw.length !== 32) throw new VaultUnavailable(`${name} must be 32 bytes, got ${raw.length}`);
    const kid = await keyId(raw);
    let key = imported.get(kid);
    if (!key) {
      key = await crypto.subtle.importKey("raw", raw, { name: "AES-GCM" }, false, ["encrypt", "decrypt"]);
      imported.set(kid, key);
    }
    return { kid, key };
  } finally {
    raw.fill(0);
  }
};

/**
 * The keyring from the Worker's secrets. `VAULT_KEK` is required; a malformed
 * `VAULT_KEK_PREVIOUS` is refused too (loudly, rather than silently stranding
 * every not-yet-migrated record). A previous key equal to the current one is
 * ignored.
 */
export const loadKeyring = async (env: Pick<Env, "VAULT_KEK" | "VAULT_KEK_PREVIOUS">): Promise<Keyring> => {
  if (!env.VAULT_KEK) throw new VaultUnavailable("VAULT_KEK is not set");
  const current = await loadKek("VAULT_KEK", env.VAULT_KEK);
  if (!env.VAULT_KEK_PREVIOUS) return { current };
  const previous = await loadKek("VAULT_KEK_PREVIOUS", env.VAULT_KEK_PREVIOUS);
  return previous.kid === current.kid ? { current } : { current, previous };
};

export const seal = async (keyring: Keyring, plaintext: string, aad: string): Promise<string> => {
  const { kid, key } = keyring.current;
  const iv = crypto.getRandomValues(new Uint8Array(12));
  const ciphertext = await crypto.subtle.encrypt(
    { name: "AES-GCM", iv, additionalData: utf8(aad), tagLength: 128 },
    key,
    utf8(plaintext)
  );
  return `${ENVELOPE_VERSION}.${kid}.${toBase64Url(iv)}.${toBase64Url(new Uint8Array(ciphertext))}`;
};

/** Throws `UnknownKeyError` for a kid the keyring lacks; any other failure
 * (malformed envelope, tampered bytes, AAD mismatch) throws a plain Error. */
export const open = async (keyring: Keyring, envelope: string, aad: string): Promise<Opened> => {
  const [version, kid, iv, ciphertext, extra] = envelope.split(".");
  if (version !== ENVELOPE_VERSION || !kid || !KID_RE.test(kid) || !iv || !ciphertext || extra !== undefined) {
    throw new Error("unrecognized envelope");
  }
  const kek =
    kid === keyring.current.kid ? keyring.current : kid === keyring.previous?.kid ? keyring.previous : undefined;
  if (!kek) throw new UnknownKeyError(kid);
  const plaintext = await crypto.subtle.decrypt(
    { name: "AES-GCM", iv: fromBase64Url(iv), additionalData: utf8(aad), tagLength: 128 },
    kek.key,
    fromBase64Url(ciphertext)
  );
  return { plaintext: fromUtf8(new Uint8Array(plaintext)), kid, rewrap: kek !== keyring.current };
};

export const sealJson = (keyring: Keyring, value: unknown, aad: string): Promise<string> =>
  seal(keyring, JSON.stringify(value), aad);

export const openJson = async <T>(
  keyring: Keyring,
  envelope: string,
  aad: string
): Promise<{ readonly value: T; readonly opened: Opened }> => {
  const opened = await open(keyring, envelope, aad);
  return { value: JSON.parse(opened.plaintext) as T, opened };
};

/** The kid an envelope names, without opening it (ops/tests). */
export const envelopeKid = (envelope: string): string | undefined => {
  const kid = envelope.split(".")[1];
  return kid && KID_RE.test(kid) ? kid : undefined;
};
