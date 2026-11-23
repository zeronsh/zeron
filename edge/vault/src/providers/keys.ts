/**
 * Static API keys (`*-key`): the user's own key for one vendor, pasted on the
 * laptop and uploaded once. Nothing to refresh; grants still expire hourly so
 * a revoke or disconnect reaches consumers within the hour.
 *
 * `cursor-key` alone may carry an expiry: Cursor's browser sign-in mints a
 * named user key that lapses. Past it the account turns `needs_reconnect`
 * (no decrypt needed to show it), and no grant outlives it.
 */
import type { VaultKeyProviderId } from "../api";
import { HOUR_MS, isObject, type ParseResult, type ProviderAdapter } from "./types";

export interface KeySecret {
  readonly key: string;
  /** Unix ms the key stops working (Cursor sign-in keys only). */
  readonly expiresAt?: number;
}

/** Real keys are ~100 printable chars; the bounds only reject garbage. */
const KEY_RE = /^[\x21-\x7e]{8,512}$/;

export const keyLabel = (key: string): string => `…${key.slice(-4)}`;

/** Expiries a key can sensibly have: after 2020, within ten years. */
const plausibleExpiry = (value: unknown, now: number): value is number =>
  typeof value === "number" && Number.isSafeInteger(value) && value > Date.UTC(2020, 0, 1) && value < now + 3650 * 24 * HOUR_MS;

export const keyAdapter = (id: VaultKeyProviderId): ProviderAdapter<KeySecret> => ({
  id,
  parseUpload: (upload, now): ParseResult<KeySecret> => {
    const material: unknown = upload;
    if (!isObject(material) || typeof material.key !== "string") {
      return { ok: false, message: `${id} material must be {key}` };
    }
    const key = material.key.trim();
    if (!KEY_RE.test(key)) return { ok: false, message: `${id} key is malformed` };
    const expiresAt = material.expiresAt;
    if (expiresAt === undefined) return { ok: true, secret: { key } };
    if (id !== "cursor-key") return { ok: false, message: `${id} keys don't expire` };
    if (!plausibleExpiry(expiresAt, now)) return { ok: false, message: `${id} expiresAt is not a plausible Unix ms time` };
    if (expiresAt <= now) return { ok: false, message: `${id} key has already expired` };
    return { ok: true, secret: { key, expiresAt } };
  },
  account: (secret) => keyLabel(secret.key),
  refreshable: () => false,
  refreshDue: () => false,
  hardExpiresAt: (secret) => secret.expiresAt,
  grant: (secret, now) => ({
    accessToken: secret.key,
    expiresAt: Math.min(now + HOUR_MS, secret.expiresAt ?? Number.MAX_SAFE_INTEGER),
    account: keyLabel(secret.key)
  })
});

export const anthropicKeyAdapter = keyAdapter("anthropic-key");
export const openaiKeyAdapter = keyAdapter("openai-key");
