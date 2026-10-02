/**
 * Runner JWTs: the Cloud device's bearer (docs/design/cloud-device.md,
 * "Runner identity"). The Cloud engine never holds a WorkOS session — WorkOS
 * refresh tokens are single-use, so sharing a laptop's would race — so the
 * edge mints its own short-lived ES256 token once the engine proves
 * possession of its enrolled Ed25519 key (`POST /runner/token`).
 *
 * Claims: `{iss: "zeron-edge", sub: userId, org_id, dev: deviceId, cld:
 * accountDeviceId, kind: "runner"}`, 1 h lifetime. `dev` is the session
 * device (one per Cloud session sandbox), `cld` the account's logical Cloud
 * device (owner of the Cloud projects). Deleting the Cloud device clears its key, so the engine goes
 * dark within one token lifetime.
 *
 * Dev mode without keys mints the dev-format bearer
 * `runner:{userId}@{orgId}:{deviceId}[:{accountDeviceId}]` instead (auth.ts accepts it only when
 * AUTH_MODE=dev), so local e2e runs need no key material.
 */
import { SignJWT, decodeJwt, importJWK, jwtVerify, type JWK } from "jose";
import type { Env } from "./env";

export const RUNNER_ISSUER = "zeron-edge";
export const RUNNER_TOKEN_TTL_MS = 60 * 60 * 1000;

export interface RunnerIdentity {
  readonly userId: string;
  readonly orgId: string;
  /** The session device. */
  readonly deviceId: string;
  /** The account's logical Cloud device (`cld`). */
  readonly accountDeviceId?: string;
}

export interface RunnerToken {
  readonly accessToken: string;
  /** Unix ms. */
  readonly expiresAt: number;
}

const DEV_BEARER_RE = /^runner:([^@:\s]+)@([^@:\s]+):([^@:\s]+)(?::([^@:\s]+))?$/;

export const devRunnerBearer = (id: RunnerIdentity): string =>
  `runner:${id.userId}@${id.orgId}:${id.deviceId}${id.accountDeviceId ? `:${id.accountDeviceId}` : ""}`;

export const parseDevRunnerBearer = (token: string): RunnerIdentity | undefined => {
  const m = DEV_BEARER_RE.exec(token);
  if (!m) return undefined;
  return { userId: m[1]!, orgId: m[2]!, deviceId: m[3]!, ...(m[4] ? { accountDeviceId: m[4] } : {}) };
};

// Imported keys are cached per secret value: importJWK is not free and the
// secret only changes on redeploy (which drops the isolate anyway).
const keyCache = new Map<string, Promise<{ key: CryptoKey | Uint8Array; kid?: string }>>();

const importKey = (jwkJson: string) => {
  let cached = keyCache.get(jwkJson);
  if (!cached) {
    cached = (async () => {
      const jwk = JSON.parse(jwkJson) as JWK;
      return { key: await importJWK(jwk, "ES256"), kid: jwk.kid };
    })();
    // A malformed secret must not poison the cache forever.
    cached.catch(() => keyCache.delete(jwkJson));
    keyCache.set(jwkJson, cached);
  }
  return cached;
};

/** `undefined` when this deployment cannot mint runner tokens at all
 * (production without `RUNNER_JWT_PRIVATE_KEY`). */
export const mintRunnerToken = async (
  env: Pick<Env, "RUNNER_JWT_PRIVATE_KEY" | "AUTH_MODE">,
  id: RunnerIdentity,
  now = Date.now()
): Promise<RunnerToken | undefined> => {
  const expiresAt = now + RUNNER_TOKEN_TTL_MS;
  if (!env.RUNNER_JWT_PRIVATE_KEY) {
    return env.AUTH_MODE === "dev" ? { accessToken: devRunnerBearer(id), expiresAt } : undefined;
  }
  const { key, kid } = await importKey(env.RUNNER_JWT_PRIVATE_KEY);
  const claims = { org_id: id.orgId, dev: id.deviceId, ...(id.accountDeviceId ? { cld: id.accountDeviceId } : {}), kind: "runner" };
  const accessToken = await new SignJWT(claims)
    .setProtectedHeader(kid ? { alg: "ES256", kid } : { alg: "ES256" })
    .setIssuer(RUNNER_ISSUER)
    .setSubject(id.userId)
    .setIssuedAt(Math.floor(now / 1000))
    .setExpirationTime(Math.floor(expiresAt / 1000))
    .sign(key);
  return { accessToken, expiresAt };
};

/** Cheap pre-check (no signature work): does this bearer claim to be ours?
 * WorkOS tokens carry WorkOS's issuer, so routing on `iss` is unambiguous. */
export const claimsRunnerIssuer = (token: string): boolean => {
  try {
    return decodeJwt(token).iss === RUNNER_ISSUER;
  } catch {
    return false;
  }
};

export const verifyRunnerJwt = async (
  publicKeyJwk: string,
  token: string
): Promise<RunnerIdentity | undefined> => {
  try {
    const { key } = await importKey(publicKeyJwk);
    const { payload } = await jwtVerify(token, key, {
      issuer: RUNNER_ISSUER,
      algorithms: ["ES256"]
    });
    const { sub, org_id: orgId, dev: deviceId, cld, kind } = payload;
    if (kind !== "runner") return undefined;
    if (typeof sub !== "string" || typeof orgId !== "string" || typeof deviceId !== "string") {
      return undefined;
    }
    if (!sub || !orgId || !deviceId) return undefined;
    return { userId: sub, orgId, deviceId, ...(typeof cld === "string" && cld ? { accountDeviceId: cld } : {}) };
  } catch {
    return undefined;
  }
};
