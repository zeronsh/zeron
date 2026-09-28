/**
 * Apple Push Notification service client: token-based (ES256 provider JWT
 * from a `.p8` auth key) over HTTP/2 — a deployed Worker's `fetch` speaks
 * HTTP/2 to api.push.apple.com (workerd running locally on macOS does not,
 * so this path is only exercised deployed).
 */
import { SignJWT, importPKCS8 } from "jose";

export interface ApnsConfig {
  /** Contents of the AuthKey_XXXX.p8 file. */
  keyP8: string;
  keyId: string;
  teamId: string;
  /** The app's bundle id. */
  topic: string;
}

export type ApnsEnvironment = "production" | "sandbox";

export interface ApnsResult {
  status: number;
  /** APNs `reason` on failure (BadDeviceToken, Unregistered, …). */
  reason?: string;
}

const HOSTS: Record<ApnsEnvironment, string> = {
  production: "https://api.push.apple.com",
  sandbox: "https://api.sandbox.push.apple.com"
};

/** Provider tokens are valid for an hour and APNs rejects refreshing them
 * more than every 20 minutes: reuse one for 50. Per isolate. */
let cached: { keyId: string; jwt: string; at: number } | undefined;
const JWT_TTL_MS = 50 * 60 * 1000;

export const providerToken = async (cfg: ApnsConfig, now = Date.now()): Promise<string> => {
  if (cached && cached.keyId === cfg.keyId && now - cached.at < JWT_TTL_MS) return cached.jwt;
  const key = await importPKCS8(cfg.keyP8.trim(), "ES256");
  const jwt = await new SignJWT({})
    .setProtectedHeader({ alg: "ES256", kid: cfg.keyId })
    .setIssuer(cfg.teamId)
    .setIssuedAt(Math.floor(now / 1000))
    .sign(key);
  cached = { keyId: cfg.keyId, jwt, at: now };
  return jwt;
};

/** Tokens APNs says are gone for good — drop them. */
export const isDeadToken = (r: ApnsResult): boolean =>
  r.status === 410 || r.reason === "BadDeviceToken" || r.reason === "Unregistered" || r.reason === "DeviceTokenNotForTopic";

export const sendApns = async (
  cfg: ApnsConfig,
  environment: ApnsEnvironment,
  deviceToken: string,
  payload: unknown,
  collapseId: string
): Promise<ApnsResult> => {
  const jwt = await providerToken(cfg);
  const response = await fetch(`${HOSTS[environment]}/3/device/${deviceToken}`, {
    method: "POST",
    headers: {
      authorization: `bearer ${jwt}`,
      "apns-topic": cfg.topic,
      "apns-push-type": "alert",
      "apns-priority": "10",
      // Deliver within a day if the phone is off; older than that is noise.
      "apns-expiration": String(Math.floor(Date.now() / 1000) + 24 * 60 * 60),
      // APNs caps collapse ids at 64 bytes.
      "apns-collapse-id": collapseId.slice(0, 64),
      "content-type": "application/json"
    },
    body: JSON.stringify(payload)
  });
  if (response.ok) return { status: response.status };
  let reason: string | undefined;
  try {
    reason = ((await response.json()) as { reason?: string }).reason;
  } catch {
    reason = undefined;
  }
  return { status: response.status, reason };
};
