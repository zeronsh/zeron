/**
 * Hermes' xAI sign-ins (`xai-oauth`): the record `hermes auth add xai-oauth`
 * writes under `providers["xai-oauth"]` — an xAI OAuth token set with the
 * API scopes Hermes asks for — refreshed here from upload on, so the vault
 * is the only refresher of its rotating refresh token.
 *
 * Refresh: `POST {token_endpoint}` (form: `grant_type=refresh_token`,
 * `client_id`, `refresh_token`), the endpoint pinned to an `https` `x.ai`
 * host (default `https://auth.x.ai/oauth2/token`). Hermes itself starts
 * refreshing an hour before a long-lived token's expiry, so grants are kept
 * at least two hours (or half the token's life) from it.
 */
import { decodeJwtPayload } from "../jwt";
import { expiryOf, httpsUrlOn, postTokenForm, refreshDueAhead } from "./oauth";
import { HOUR_MS, isObject, nonEmptyString, type ParseResult, type ProviderAdapter, type RefreshContext, type RefreshOutcome } from "./types";

/** Hermes' public xAI client (hermes_cli/auth_xai.py). */
export const HERMES_XAI_CLIENT_ID = "b1a00492-073a-47ea-816f-4c329264a828";
export const XAI_TOKEN_URL = "https://auth.x.ai/oauth2/token";
const XAI_DOMAINS = ["x.ai"] as const;
const REFRESH_AHEAD_MS = 2 * HOUR_MS;

export interface XaiOauthSecret {
  readonly accessToken: string;
  readonly refreshToken: string;
  readonly idToken?: string;
  readonly tokenType?: string;
  readonly tokenEndpoint: string;
  readonly clientId: string;
  /** Unix ms; 0 = unknown (refreshed at the first grant). */
  readonly expiresAt: number;
  /** Unix ms of the last refresh; 0 = unknown (an upload). */
  readonly issuedAt: number;
}

export const parseXaiOauthUpload = (material: unknown, now: number): ParseResult<XaiOauthSecret> => {
  const record = isObject(material) ? material.xaiOauth : undefined;
  if (!isObject(record) || !isObject(record.tokens)) return { ok: false, message: "xai-oauth material must be {xaiOauth: {tokens}}" };
  const tokens = record.tokens;
  if (!nonEmptyString(tokens.access_token) || !nonEmptyString(tokens.refresh_token)) {
    return { ok: false, message: "xai-oauth tokens need access_token and refresh_token" };
  }
  const discovery = isObject(record.discovery) ? record.discovery : {};
  const endpoint = nonEmptyString(discovery.token_endpoint) ? discovery.token_endpoint : XAI_TOKEN_URL;
  if (!httpsUrlOn(endpoint, XAI_DOMAINS)) return { ok: false, message: "xai-oauth token endpoint must be an https x.ai url" };
  const clientId = isObject(material) && nonEmptyString(material.clientId) ? material.clientId : HERMES_XAI_CLIENT_ID;
  if (!/^[A-Za-z0-9._:-]{1,128}$/.test(clientId)) return { ok: false, message: "xai-oauth client id is malformed" };
  return {
    ok: true,
    secret: {
      accessToken: tokens.access_token,
      refreshToken: tokens.refresh_token,
      ...(nonEmptyString(tokens.id_token) ? { idToken: tokens.id_token } : {}),
      ...(nonEmptyString(tokens.token_type) ? { tokenType: tokens.token_type } : {}),
      tokenEndpoint: endpoint,
      clientId,
      expiresAt: expiryOf(now, undefined, tokens.access_token, tokens.expires_in),
      issuedAt: 0
    }
  };
};

const claims = (secret: XaiOauthSecret) =>
  decodeJwtPayload(secret.idToken ?? "") ?? decodeJwtPayload(secret.accessToken) ?? {};

export const refreshXaiOauth = async (
  secret: XaiOauthSecret,
  context: RefreshContext
): Promise<RefreshOutcome<XaiOauthSecret>> => {
  if (!httpsUrlOn(secret.tokenEndpoint, XAI_DOMAINS)) return { kind: "reconnect", reason: "token endpoint is not an xAI host" };
  const done = await postTokenForm(context.fetch, secret.tokenEndpoint, {
    grant_type: "refresh_token",
    client_id: secret.clientId,
    refresh_token: secret.refreshToken
  });
  if (done.kind !== "ok") return done;
  const { reply } = done;
  return {
    kind: "ok",
    secret: {
      ...secret,
      accessToken: reply.accessToken,
      refreshToken: reply.refreshToken ?? secret.refreshToken,
      ...(nonEmptyString(reply.body.id_token) ? { idToken: reply.body.id_token } : {}),
      expiresAt: expiryOf(context.now, undefined, reply.accessToken, reply.expiresIn ?? 3600),
      issuedAt: context.now
    }
  };
};

export const xaiOauthAdapter: ProviderAdapter<XaiOauthSecret> = {
  id: "xai-oauth",
  parseUpload: (material, now) => parseXaiOauthUpload(material, now),
  account: (secret) => {
    const email = claims(secret).email;
    return typeof email === "string" && email ? email : "xAI account";
  },
  identity: async (secret, profile) => {
    const c = claims(secret);
    const who = (typeof c.sub === "string" && c.sub) || (typeof c.email === "string" && c.email) || profile.email;
    return who ? `xai:${who.toLowerCase()}` : undefined;
  },
  refreshable: () => true,
  refreshDue: (secret, now) => refreshDueAhead(secret.expiresAt, secret.issuedAt, now, REFRESH_AHEAD_MS),
  refresh: refreshXaiOauth,
  grant: (secret) => ({
    accessToken: secret.accessToken,
    expiresAt: secret.expiresAt,
    settings: {
      token_endpoint: secret.tokenEndpoint,
      ...(secret.tokenType ? { token_type: secret.tokenType } : {}),
      ...(secret.idToken ? { id_token: secret.idToken } : {})
    }
  })
};
