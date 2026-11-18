/**
 * Muse Code's Meta sign-ins (`muse`): the whole `auth.json` Muse writes
 * (file-backed) — its `providers.meta` login (`access_token`, `expires_at` in
 * Unix seconds) beside whatever else the document holds. Muse refreshes no
 * OAuth token: it mints its model keys from the access token, which lasts
 * until `expires_at`. So the vault keeps the document, never refreshes it,
 * and turns the account `needs_reconnect` at its expiry; machines get the
 * document back with every refresh token emptied, and grants expire hourly
 * so a disconnect reaches them within the hour.
 */
import { keyId } from "../crypto";
import { utf8 } from "../encoding";
import { HOUR_MS, isObject, nonEmptyString, type ParseResult, type ProviderAdapter } from "./types";

const MAX_DOCUMENT = 32 * 1024;

export interface MuseSecret {
  readonly document: Record<string, unknown>;
  readonly accessToken: string;
  /** Unix ms. */
  readonly expiresAt: number;
  readonly email?: string;
}

/** `value` with every `refresh_token` string emptied, however deep. */
export const withoutRefreshTokens = (value: unknown): unknown => {
  if (Array.isArray(value)) return value.map(withoutRefreshTokens);
  if (!isObject(value)) return value;
  return Object.fromEntries(
    Object.entries(value).map(([key, field]) => [
      key,
      key === "refresh_token" && typeof field === "string" ? "" : withoutRefreshTokens(field)
    ])
  );
};

export const parseMuseUpload = (material: unknown, now: number): ParseResult<MuseSecret> => {
  const document = isObject(material) ? material.museAuth : undefined;
  if (!isObject(document)) return { ok: false, message: "muse material must be {museAuth}" };
  if (JSON.stringify(document).length > MAX_DOCUMENT) return { ok: false, message: "muse auth.json is too large" };
  const providers = isObject(document.providers) ? document.providers : undefined;
  const meta = providers && isObject(providers.meta) ? providers.meta : undefined;
  if (!meta || !nonEmptyString(meta.access_token)) return { ok: false, message: "muse auth.json holds no Meta login" };
  const seconds = meta.expires_at;
  if (typeof seconds !== "number" || !Number.isFinite(seconds) || seconds <= 0) {
    return { ok: false, message: "muse login has no expires_at" };
  }
  const expiresAt = seconds * 1000;
  if (expiresAt <= now) return { ok: false, message: "muse login has already expired" };
  return {
    ok: true,
    secret: {
      document,
      accessToken: meta.access_token,
      expiresAt,
      ...(nonEmptyString(meta.user_email) ? { email: meta.user_email } : {})
    }
  };
};

export const museAdapter: ProviderAdapter<MuseSecret> = {
  id: "muse",
  parseUpload: (material, now) => parseMuseUpload(material, now),
  account: (secret) => secret.email ?? "Meta account",
  identity: async (secret, profile) => {
    const email = secret.email ?? profile.email;
    return email ? `meta:${email.toLowerCase()}` : `meta:key:${await keyId(utf8(secret.accessToken))}`;
  },
  refreshable: () => false,
  refreshDue: () => false,
  hardExpiresAt: (secret) => secret.expiresAt,
  grant: (secret, now) => ({
    accessToken: secret.accessToken,
    expiresAt: Math.min(now + HOUR_MS, secret.expiresAt),
    ...(secret.email ? { account: secret.email } : {}),
    settings: { document: JSON.stringify(withoutRefreshTokens(secret.document)) }
  })
};
