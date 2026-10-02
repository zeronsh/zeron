/**
 * GitHub App installation tokens — what a Cloud session gets for `git` and
 * `gh`. The user-to-server token from the device flow (github.ts) never
 * leaves the vault; it only proves which installations the user can reach.
 *
 * A grant for repository `owner/name`:
 * 1. With the user token, list `/user/installations` (installations of this
 *    App the user can access) and pick the one whose account is `owner`.
 *    This is the ownership check: an installation the user can't reach is
 *    never minted for, whatever id a caller names.
 * 2. With an App JWT (RS256, signed by `GITHUB_APP_PRIVATE_KEY`, `iss` = the
 *    client id), `POST /app/installations/{id}/access_tokens`: a one-hour
 *    token for every repository of that installation, acting as the App.
 *
 * GitHub downloads App keys as PKCS#1 (`BEGIN RSA PRIVATE KEY`); WebCrypto
 * only imports PKCS#8, so a PKCS#1 key is wrapped in a PrivateKeyInfo first.
 */
import { fromBase64, toBase64Url, utf8 } from "../encoding";
import { GITHUB_API, nextPage } from "./github-repos";
import { isObject, nonEmptyString, upstream, type FetchFn } from "./types";

/** GitHub caps App JWTs at 10 minutes; `iat` is backdated for clock drift. */
const APP_JWT_TTL_SECS = 9 * 60;
const APP_JWT_BACKDATE_SECS = 60;
/** Installation lists are short; bounds a pathological account. */
const MAX_INSTALLATION_PAGES = 10;
/** `owner/name` as GitHub allows them. */
const REPO_RE = /^[A-Za-z0-9](?:[A-Za-z0-9-]{0,38})\/[A-Za-z0-9._-]{1,100}$/;

const RSA_ALGORITHM_IDENTIFIER = [0x30, 0x0d, 0x06, 0x09, 0x2a, 0x86, 0x48, 0x86, 0xf7, 0x0d, 0x01, 0x01, 0x01, 0x05, 0x00];

export const isRepoFullName = (value: unknown): value is string => typeof value === "string" && REPO_RE.test(value);

export const installUrl = (slug: string): string =>
  `https://github.com/apps/${encodeURIComponent(slug)}/installations/new`;

const derLength = (length: number): number[] => {
  if (length < 0x80) return [length];
  const bytes: number[] = [];
  for (let rest = length; rest > 0; rest >>>= 8) bytes.unshift(rest & 0xff);
  return [0x80 | bytes.length, ...bytes];
};

const der = (tag: number, body: ArrayLike<number>): number[] => [tag, ...derLength(body.length), ...Array.from(body)];

/** PEM (PKCS#8 or GitHub's PKCS#1) → PKCS#8 DER. Throws on anything else.
 * Literal `\n` escapes (a PEM squeezed onto one env-file line) count as
 * line breaks. */
export const pkcs8Der = (pem: string): Uint8Array<ArrayBuffer> => {
  const body = fromBase64(
    pem
      .replace(/\\n/g, "\n")
      .replace(/-----(BEGIN|END) [A-Z ]+-----/g, "")
      .replace(/\s+/g, "")
  );
  if (pem.includes("BEGIN PRIVATE KEY")) return body;
  if (!pem.includes("BEGIN RSA PRIVATE KEY")) throw new Error("GitHub App key must be a PEM RSA private key");
  const info = der(0x30, [0x02, 0x01, 0x00, ...RSA_ALGORITHM_IDENTIFIER, ...der(0x04, body)]);
  return Uint8Array.from(info);
};

/** Imported keys, per isolate, by PEM: a rotated secret imports afresh. */
const keys = new Map<string, Promise<CryptoKey>>();

const signingKey = (pem: string): Promise<CryptoKey> => {
  let key = keys.get(pem);
  if (!key) {
    key = crypto.subtle.importKey("pkcs8", pkcs8Der(pem), { name: "RSASSA-PKCS1-v1_5", hash: "SHA-256" }, false, [
      "sign"
    ]);
    key.catch(() => keys.delete(pem));
    keys.clear();
    keys.set(pem, key);
  }
  return key;
};

/** An App JWT (`iss` = client id, which GitHub accepts in place of the app id). */
export const appJwt = async (clientId: string, pem: string, nowMs: number): Promise<string> => {
  const now = Math.floor(nowMs / 1000);
  const header = toBase64Url(utf8(JSON.stringify({ alg: "RS256", typ: "JWT" })));
  const claims = toBase64Url(
    utf8(JSON.stringify({ iat: now - APP_JWT_BACKDATE_SECS, exp: now + APP_JWT_TTL_SECS, iss: clientId }))
  );
  const signature = await crypto.subtle.sign("RSASSA-PKCS1-v1_5", await signingKey(pem), utf8(`${header}.${claims}`));
  return `${header}.${claims}.${toBase64Url(new Uint8Array(signature))}`;
};

const headers = (bearer: string) => ({
  accept: "application/vnd.github+json",
  authorization: `Bearer ${bearer}`,
  "user-agent": "zeron-vault",
  "x-github-api-version": "2022-11-28"
});

export type InstallationLookup =
  | { readonly kind: "found"; readonly id: number }
  /** No usable installation of the App on `owner` that the user can reach. */
  | { readonly kind: "none" }
  /** GitHub answered 401: the user token is dead. */
  | { readonly kind: "unauthorized" }
  | { readonly kind: "failed"; readonly reason: string };

/** The installation on `owner` among the user's installations (case-insensitive;
 * suspended ones don't count). */
export const findInstallation = async (
  userToken: string,
  owner: string,
  fetchFn: FetchFn
): Promise<InstallationLookup> => {
  const wanted = owner.toLowerCase();
  let url: string | undefined = `${GITHUB_API}/user/installations?per_page=100`;
  for (let page = 0; url && page < MAX_INSTALLATION_PAGES; page++) {
    const response = await upstream(fetchFn, url, { headers: headers(userToken) });
    if (!response) return { kind: "failed", reason: "timeout or network error" };
    if (response.status === 401) return { kind: "unauthorized" };
    if (!response.ok) return { kind: "failed", reason: `GitHub HTTP ${response.status}` };
    const body: unknown = await response.json().catch(() => undefined);
    if (!isObject(body) || !Array.isArray(body.installations)) {
      return { kind: "failed", reason: "unexpected GitHub response" };
    }
    for (const item of body.installations) {
      if (!isObject(item) || typeof item.id !== "number" || !isObject(item.account)) continue;
      const login = item.account.login;
      if (typeof login === "string" && login.toLowerCase() === wanted && !item.suspended_at) {
        return { kind: "found", id: item.id };
      }
    }
    url = nextPage(response.headers.get("link"));
  }
  return { kind: "none" };
};

export type InstallationToken =
  | { readonly kind: "ok"; readonly token: string; readonly expiresAt: number }
  /** 404: the App was uninstalled (or the installation never existed). */
  | { readonly kind: "gone" }
  /** 401/403: our App credentials are wrong — never the user's fault. */
  | { readonly kind: "rejected"; readonly reason: string }
  | { readonly kind: "failed"; readonly reason: string };

export const mintInstallationToken = async (
  jwt: string,
  installationId: number,
  fetchFn: FetchFn
): Promise<InstallationToken> => {
  const response = await upstream(fetchFn, `${GITHUB_API}/app/installations/${installationId}/access_tokens`, {
    method: "POST",
    headers: headers(jwt)
  });
  if (!response) return { kind: "failed", reason: "timeout or network error" };
  if (response.status === 404) return { kind: "gone" };
  if (response.status === 401 || response.status === 403) {
    return { kind: "rejected", reason: `GitHub HTTP ${response.status}` };
  }
  if (!response.ok) return { kind: "failed", reason: `GitHub HTTP ${response.status}` };
  const body: unknown = await response.json().catch(() => undefined);
  const expiresAt = isObject(body) && typeof body.expires_at === "string" ? Date.parse(body.expires_at) : Number.NaN;
  if (!isObject(body) || !nonEmptyString(body.token) || !Number.isFinite(expiresAt)) {
    return { kind: "failed", reason: "unexpected GitHub response" };
  }
  return { kind: "ok", token: body.token, expiresAt };
};

/** Canary: do the App credentials still sign in? (`GET /app` with the JWT.) */
export const checkApp = async (clientId: string, pem: string, fetchFn: FetchFn): Promise<boolean> => {
  let jwt: string;
  try {
    jwt = await appJwt(clientId, pem, Date.now());
  } catch {
    return false;
  }
  const response = await upstream(fetchFn, `${GITHUB_API}/app`, { headers: headers(jwt) });
  return response?.ok === true;
};
