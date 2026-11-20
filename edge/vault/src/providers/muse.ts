/**
 * Muse Code's Meta sign-ins (`muse`): the whole `auth.json` Muse writes
 * (file-backed). Its `providers.meta` login, as Muse 1.4 writes it, is
 * `{access_token, api_key, api_base_url, obtained_via, mechanism,
 * user_full_name, user_email}`: no refresh token and no expiry — Muse mints
 * its model keys (`api_key`) from the access token and refreshes nothing.
 * So the vault keeps the document as it came and never refreshes it; an
 * expiry, where the login names one (`expires_at` Unix seconds, or the
 * token's JWT `exp`), turns the account `needs_reconnect` then. Machines get
 * the document back with any refresh token emptied, and grants expire
 * hourly so a disconnect reaches them within the hour.
 */
import { keyId } from "../crypto";
import { utf8 } from "../encoding";
import { jwtExpMs } from "../jwt";
import { HOUR_MS, isObject, nonEmptyString, type ParseResult, type ProviderAdapter } from "./types";

const MAX_DOCUMENT = 32 * 1024;

export interface MuseSecret {
  readonly document: Record<string, unknown>;
  readonly accessToken: string;
  /** Unix ms; absent when the login names no expiry (Muse 1.4's don't). */
  readonly expiresAt?: number;
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
  const expiresAt =
    typeof seconds === "number" && Number.isFinite(seconds) && seconds > 0 ? seconds * 1000 : jwtExpMs(meta.access_token);
  if (expiresAt !== undefined && expiresAt <= now) return { ok: false, message: "muse login has already expired" };
  return {
    ok: true,
    secret: {
      document,
      accessToken: meta.access_token,
      ...(expiresAt !== undefined ? { expiresAt } : {}),
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
    expiresAt: Math.min(now + HOUR_MS, secret.expiresAt ?? Number.MAX_SAFE_INTEGER),
    ...(secret.email ? { account: secret.email } : {}),
    settings: { document: JSON.stringify(withoutRefreshTokens(secret.document)) }
  })
};
