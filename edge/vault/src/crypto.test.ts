import { afterEach, describe, expect, it } from "vitest";
import {
  UnknownKeyError,
  VaultUnavailable,
  clearKeyCache,
  envelopeKid,
  keyId,
  loadKeyring,
  open,
  openJson,
  recordAad,
  seal,
  sealJson
} from "./crypto";
import { fromBase64Url, toBase64, toBase64Url, toHex } from "./encoding";

const rawKey = (fill: number) => new Uint8Array(32).fill(fill);
const secret = (fill: number) => toBase64(rawKey(fill));
const ring = (current: number, previous?: number) =>
  loadKeyring({ VAULT_KEK: secret(current), VAULT_KEK_PREVIOUS: previous === undefined ? undefined : secret(previous) });

afterEach(() => clearKeyCache());

describe("key ids", () => {
  it("kid is the first 16 hex chars of SHA-256(raw key)", async () => {
    const digest = toHex(await crypto.subtle.digest("SHA-256", rawKey(7)));
    expect(await keyId(rawKey(7))).toBe(digest.slice(0, 16));
    expect((await ring(7)).current.kid).toBe(digest.slice(0, 16));
  });

  it("keeps keys non-extractable and caches them per kid", async () => {
    const a = await ring(7);
    const b = await ring(7);
    expect(a.current.key.extractable).toBe(false);
    expect(b.current.key).toBe(a.current.key);
  });
});

describe("keyring secrets", () => {
  it("VAULT_KEK is required, base64, 32 bytes", async () => {
    await expect(loadKeyring({})).rejects.toBeInstanceOf(VaultUnavailable);
    await expect(loadKeyring({ VAULT_KEK: "not base64!" })).rejects.toThrow(/VAULT_KEK is not valid base64/);
    await expect(loadKeyring({ VAULT_KEK: toBase64(new Uint8Array(16)) })).rejects.toThrow(/32 bytes/);
  });

  it("never caches a failure: fixing the secret recovers at once", async () => {
    await expect(loadKeyring({ VAULT_KEK: "" })).rejects.toBeInstanceOf(VaultUnavailable);
    await expect(ring(3)).resolves.toBeDefined();
  });

  it("a malformed VAULT_KEK_PREVIOUS is refused, not ignored", async () => {
    await expect(loadKeyring({ VAULT_KEK: secret(1), VAULT_KEK_PREVIOUS: "AAAA" })).rejects.toThrow(/VAULT_KEK_PREVIOUS/);
  });

  it("a previous key equal to the current one is dropped", async () => {
    expect((await ring(1, 1)).previous).toBeUndefined();
  });
});

describe("AES-GCM envelope", () => {
  it("round-trips under the same AAD and names its key", async () => {
    const keyring = await ring(7);
    const aad = recordAad("user_1", "codex", 3);
    expect(aad).toBe("user_1|codex|3");
    const envelope = await sealJson(keyring, { refresh: "rt-secret" }, aad);
    expect(envelope).toMatch(/^v2\.[0-9a-f]{16}\.[A-Za-z0-9_-]{16}\.[A-Za-z0-9_-]+$/);
    expect(envelopeKid(envelope)).toBe(keyring.current.kid);
    expect(envelope).not.toContain("rt-secret");
    const { value, opened } = await openJson(keyring, envelope, aad);
    expect(value).toEqual({ refresh: "rt-secret" });
    expect(opened).toMatchObject({ kid: keyring.current.kid, rewrap: false });
  });

  it("uses a fresh IV per seal", async () => {
    const keyring = await ring(7);
    const a = await seal(keyring, "same", "aad");
    const b = await seal(keyring, "same", "aad");
    expect(a.split(".")[2]).not.toBe(b.split(".")[2]);
  });

  it("refuses another slot's or generation's AAD", async () => {
    const keyring = await ring(7);
    const envelope = await seal(keyring, "token", recordAad("user_1", "codex", 3));
    await expect(open(keyring, envelope, recordAad("user_2", "codex", 3))).rejects.toThrow();
    await expect(open(keyring, envelope, recordAad("user_1", "github", 3))).rejects.toThrow();
    await expect(open(keyring, envelope, recordAad("user_1", "codex", 2))).rejects.toThrow();
  });

  it("refuses tampered ciphertext and unknown versions", async () => {
    const keyring = await ring(1);
    const aad = recordAad("u", "codex", 1);
    const envelope = await seal(keyring, "token", aad);
    const [version, kid, iv, ciphertext] = envelope.split(".");
    const bytes = fromBase64Url(ciphertext!);
    bytes[0]! ^= 1;
    await expect(open(keyring, `${version}.${kid}.${iv}.${toBase64Url(bytes)}`, aad)).rejects.toThrow();
    await expect(open(keyring, `v1.${iv}.${ciphertext}`, aad)).rejects.toThrow(/unrecognized envelope/);
    await expect(open(keyring, `v3.${kid}.${iv}.${ciphertext}`, aad)).rejects.toThrow(/unrecognized envelope/);
  });
});

describe("rotation", () => {
  it("A → (B, previous A) → B: opens, flags re-wrap, and strands only untouched records", async () => {
    const aad = recordAad("u", "claude", 4);
    const a = await ring(0xa);
    const sealedA = await seal(a, "token", aad);
    const untouched = await seal(a, "never accessed", aad);

    // Rotate: B current, A previous. A's envelope opens and asks for a re-wrap.
    const rotated = await ring(0xb, 0xa);
    const opened = await open(rotated, sealedA, aad);
    expect(opened).toEqual({ plaintext: "token", kid: a.current.kid, rewrap: true });
    const rewrapped = await seal(rotated, opened.plaintext, aad);
    expect(envelopeKid(rewrapped)).toBe(rotated.current.kid);

    // Retire A: the re-wrapped record still opens; the untouched one is stranded.
    const bOnly = await ring(0xb);
    expect(await open(bOnly, rewrapped, aad)).toMatchObject({ plaintext: "token", rewrap: false });
    const stranded = open(bOnly, untouched, aad);
    await expect(stranded).rejects.toBeInstanceOf(UnknownKeyError);
    await expect(stranded).rejects.toBeInstanceOf(VaultUnavailable);
    await expect(stranded).rejects.toThrow(`record sealed under key ${a.current.kid}, which is no longer configured`);
  });

  it("a kid that names the wrong configured key does not open", async () => {
    const aad = recordAad("u", "codex", 1);
    const keyring = await ring(0xb, 0xa);
    const envelope = await seal(keyring, "token", aad);
    const [version, , iv, ciphertext] = envelope.split(".");
    const relabeled = `${version}.${keyring.previous!.kid}.${iv}.${ciphertext}`;
    const failure = open(keyring, relabeled, aad);
    await expect(failure).rejects.toThrow();
    await expect(failure).rejects.not.toBeInstanceOf(VaultUnavailable);
  });
});
