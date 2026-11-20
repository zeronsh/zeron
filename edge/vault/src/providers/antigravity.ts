/**
 * Antigravity's Google sign-ins (`antigravity`): the `acp_token.json` its ACP
 * server writes (`client_id`, `client_secret`, `refresh_token`, `scopes`,
 * `project_id`) — it holds no access token. The vault refreshes it at
 * Google's token endpoint (Google refresh tokens don't rotate), always there
 * whatever `token_uri` the file names.
 *
 * Antigravity reads its token file once per connection and, when the access
 * token runs out with no refresh token, starts an interactive sign-in. So
 * every grant carries a token with most of its hour left: refreshed once
 * under 50 minutes remain, and the machine starts a new runtime per token.
 */
import { keyId } from "../crypto";
import { utf8 } from "../encoding";
import { postTokenForm } from "./oauth";
import { isObject, nonEmptyString, type ParseResult, type ProviderAdapter, type RefreshContext, type RefreshOutcome } from "./types";

export const GOOGLE_TOKEN_URL = "https://oauth2.googleapis.com/token";
const REFRESH_BEFORE_EXPIRY_MS = 50 * 60_000;
const CLIENT_RE = /^[A-Za-z0-9._-]{1,200}\.apps\.googleusercontent\.com$/;

export interface AntigravitySecret {
  readonly refreshToken: string;
  readonly clientId: string;
  readonly clientSecret: string;
  readonly scopes: readonly string[];
  readonly projectId?: string;
  /** Absent until the first refresh (the uploaded file holds none). */
  readonly accessToken?: string;
  /** Unix ms; 0 until the first refresh. */
  readonly expiresAt: number;
}

export const parseAntigravityUpload = (material: unknown): ParseResult<AntigravitySecret> => {
  const token = isObject(material) ? material.antigravityToken : undefined;
  if (!isObject(token)) return { ok: false, message: "antigravity material must be {antigravityToken}" };
  if (!nonEmptyString(token.refresh_token) || !nonEmptyString(token.client_id) || typeof token.client_secret !== "string") {
    return { ok: false, message: "antigravity token needs refresh_token, client_id and client_secret" };
  }
  if (!CLIENT_RE.test(token.client_id)) return { ok: false, message: "antigravity client_id is not a Google OAuth client" };
  const scopes = Array.isArray(token.scopes) ? token.scopes.filter((s): s is string => typeof s === "string" && s.length <= 200) : [];
  const projectId = typeof token.project_id === "string" && token.project_id.length <= 200 ? token.project_id : undefined;
  return {
    ok: true,
    secret: {
      refreshToken: token.refresh_token,
      clientId: token.client_id,
      clientSecret: token.client_secret,
      scopes: scopes.slice(0, 32),
      ...(projectId ? { projectId } : {}),
      expiresAt: 0
    }
  };
};

export const refreshAntigravity = async (
  secret: AntigravitySecret,
  context: RefreshContext
): Promise<RefreshOutcome<AntigravitySecret>> => {
  const done = await postTokenForm(context.fetch, GOOGLE_TOKEN_URL, {
    grant_type: "refresh_token",
    client_id: secret.clientId,
    client_secret: secret.clientSecret,
    refresh_token: secret.refreshToken
  });
  if (done.kind !== "ok") return done;
  return {
    kind: "ok",
    secret: {
      ...secret,
      accessToken: done.reply.accessToken,
      // Google keeps the refresh token; if it ever sends a new one, keep that.
      refreshToken: done.reply.refreshToken ?? secret.refreshToken,
      expiresAt: context.now + (done.reply.expiresIn ?? 3600) * 1000
    }
  };
};

export const antigravityAdapter: ProviderAdapter<AntigravitySecret> = {
  id: "antigravity",
  parseUpload: (material) => parseAntigravityUpload(material),
  account: () => "Google account",
  identity: async (secret, profile) =>
    profile.email ? `google:${profile.email.toLowerCase()}` : `google:key:${await keyId(utf8(secret.refreshToken))}`,
  refreshable: () => true,
  refreshDue: (secret, now) => !secret.accessToken || secret.expiresAt - now < REFRESH_BEFORE_EXPIRY_MS,
  refresh: refreshAntigravity,
  grant: (secret) => ({
    accessToken: secret.accessToken ?? "",
    expiresAt: secret.expiresAt,
    settings: {
      client_id: secret.clientId,
      scopes: secret.scopes.join(" "),
      ...(secret.projectId ? { project_id: secret.projectId } : {})
    }
  })
};
