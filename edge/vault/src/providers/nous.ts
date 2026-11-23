/**
 * Hermes' Nous Portal sign-ins (`nous`): the record Hermes keeps under
 * `providers.nous`, refreshed here from upload on (Nous refresh tokens are
 * single-use: reusing one revokes the whole session, so the vault must be
 * the only refresher).
 *
 * Refresh: `POST {portal_base_url}/api/oauth/token` (form:
 * `grant_type=refresh_token`, `client_id`), the refresh token in the
 * `x-nous-refresh-token` header (hermes_cli/auth_nous.py). The portal is
 * pinned to an `https` `nousresearch.com` host.
 */
import { decodeJwtPayload } from "../jwt";
import { expiryOf, httpsUrlOn, postTokenForm, refreshDueAhead } from "./oauth";
import { HOUR_MS, isObject, nonEmptyString, type ParseResult, type ProviderAdapter, type RefreshContext, type RefreshOutcome } from "./types";

export const NOUS_PORTAL = "https://portal.nousresearch.com";
export const NOUS_CLIENT_ID = "hermes-cli";
const NOUS_DOMAINS = ["nousresearch.com"] as const;
const REFRESH_AHEAD_MS = 2 * HOUR_MS;
/** The record's fields a machine writes back beside the tokens. */
const KEPT = ["portal_base_url", "inference_base_url", "client_id", "scope", "token_type"] as const;

export interface NousSecret {
  readonly accessToken: string;
  readonly refreshToken: string;
  readonly expiresAt: number;
  readonly issuedAt: number;
  readonly portal: string;
  readonly clientId: string;
  readonly fields: Readonly<Record<string, string>>;
}

const base = (url: string): string => url.replace(/\/+$/, "");

export const parseNousUpload = (material: unknown, now: number): ParseResult<NousSecret> => {
  const record = isObject(material) ? material.nousAuth : undefined;
  if (!isObject(record)) return { ok: false, message: "nous material must be {nousAuth}" };
  if (!nonEmptyString(record.access_token) || !nonEmptyString(record.refresh_token)) {
    return { ok: false, message: "nous record needs access_token and refresh_token" };
  }
  const portal = base(nonEmptyString(record.portal_base_url) ? record.portal_base_url : NOUS_PORTAL);
  if (!httpsUrlOn(portal, NOUS_DOMAINS)) return { ok: false, message: "nous portal must be an https nousresearch.com url" };
  const inference = record.inference_base_url;
  if (nonEmptyString(inference) && !httpsUrlOn(inference.replace(/\/v\d+\/?$/, ""), NOUS_DOMAINS)) {
    return { ok: false, message: "nous inference url must be an https nousresearch.com url" };
  }
  const fields: Record<string, string> = {};
  for (const key of KEPT) {
    const value = record[key];
    if (typeof value === "string" && value.length <= 512) fields[key] = value;
  }
  fields.portal_base_url = portal;
  const clientId = fields.client_id || NOUS_CLIENT_ID;
  fields.client_id = clientId;
  return {
    ok: true,
    secret: {
      accessToken: record.access_token,
      refreshToken: record.refresh_token,
      expiresAt: expiryOf(now, record.expires_at, record.access_token, record.expires_in),
      issuedAt: 0,
      portal,
      clientId,
      fields
    }
  };
};

export const refreshNous = async (secret: NousSecret, context: RefreshContext): Promise<RefreshOutcome<NousSecret>> => {
  if (!httpsUrlOn(secret.portal, NOUS_DOMAINS)) return { kind: "reconnect", reason: "portal is not a Nous host" };
  const done = await postTokenForm(
    context.fetch,
    `${secret.portal}/api/oauth/token`,
    { grant_type: "refresh_token", client_id: secret.clientId },
    { "x-nous-refresh-token": secret.refreshToken }
  );
  if (done.kind !== "ok") return done;
  const { reply } = done;
  return {
    kind: "ok",
    secret: {
      ...secret,
      accessToken: reply.accessToken,
      refreshToken: reply.refreshToken ?? secret.refreshToken,
      expiresAt: expiryOf(context.now, reply.body.expires_at, reply.accessToken, reply.expiresIn ?? 3600),
      issuedAt: context.now,
      fields: {
        ...secret.fields,
        ...(nonEmptyString(reply.body.scope) ? { scope: reply.body.scope } : {}),
        ...(nonEmptyString(reply.body.token_type) ? { token_type: reply.body.token_type } : {})
      }
    }
  };
};

const who = (secret: NousSecret): string | undefined => {
  const c = decodeJwtPayload(secret.accessToken) ?? {};
  return (typeof c.email === "string" && c.email) || (typeof c.sub === "string" && c.sub) || undefined;
};

export const nousAdapter: ProviderAdapter<NousSecret> = {
  id: "nous",
  parseUpload: (material, now) => parseNousUpload(material, now),
  account: (secret) => who(secret) ?? "Nous Portal account",
  identity: async (secret, profile) => {
    const id = who(secret) ?? profile.email;
    return id ? `nous:${id.toLowerCase()}` : undefined;
  },
  refreshable: () => true,
  refreshDue: (secret, now) => refreshDueAhead(secret.expiresAt, secret.issuedAt, now, REFRESH_AHEAD_MS),
  refresh: refreshNous,
  grant: (secret) => ({ accessToken: secret.accessToken, expiresAt: secret.expiresAt, settings: { ...secret.fields } })
};
