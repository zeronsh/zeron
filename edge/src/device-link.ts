/**
 * Which device is dialing another device's relay room (`/device/{target}/ws`,
 * client role). The bearer names only the user, and every device of an
 * account holds one, so a device id in the query is just a claim. The edge
 * stamps a calling device only when it can prove it:
 *
 * - A runner bearer (a Cloud device): its JWT's verified device id.
 * - A user bearer: the dial carries
 *     callerDevice=<id>&callerKey=<b64url plugin key>&callerTs=<epoch ms>
 *     &callerProof=<b64url ed25519 signature over
 *       "zeron-device-link:v1:" + target + ":" + connId + ":" + callerTs>
 *   The time must be within `MAX_SKEW_MS`. The key must be registered to the
 *   bearer's own user and not revoked (plugin-keys.ts; binding a new one needs
 *   a fresh sign-in), with `callerDevice` as its registered device.
 *
 * Anything missing or wrong leaves the dial unstamped, not refused: old
 * engines keep their relay, and a receiver treats an unstamped caller as an
 * unknown device. A stamped dial's 101 carries `x-zeron-caller-device`, and
 * every frame it relays carries `fromDevice` (device-room.ts).
 */
import type { Verified } from "./auth";
import { parseEd25519PublicKey, verifyEd25519 } from "./cloud/crypto";
import type { Env } from "./env";
import { forward } from "./forward";
import { userDirectoryStub } from "./user-directory";

export const DEVICE_LINK_PREFIX = "zeron-device-link:v1:";
/** How far a dial's `callerTs` may be from the edge's clock. */
export const MAX_SKEW_MS = 5 * 60_000;

const ID_RE = /^[A-Za-z0-9_-]{1,128}$/;

/** The message a dialing device signs. */
export const deviceLinkMessage = (target: string, connId: string, ts: number): string =>
  `${DEVICE_LINK_PREFIX}${target}:${connId}:${ts}`;

/** The verified calling device of a client dial into `target`'s room as
 * `connId`, or undefined. */
export const callerDevice = async (
  env: Env,
  auth: Verified,
  url: URL,
  target: string,
  connId: string
): Promise<string | undefined> => {
  if (auth.kind === "runner") return auth.deviceId && ID_RE.test(auth.deviceId) ? auth.deviceId : undefined;
  const device = url.searchParams.get("callerDevice");
  const key = url.searchParams.get("callerKey");
  const tsRaw = url.searchParams.get("callerTs");
  const proof = url.searchParams.get("callerProof");
  if (!device || !ID_RE.test(device) || !key || !parseEd25519PublicKey(key) || !tsRaw || !proof) return undefined;
  if (!/^\d{1,16}$/.test(tsRaw) || !ID_RE.test(connId)) return undefined;
  const ts = Number(tsRaw);
  if (Math.abs(Date.now() - ts) > MAX_SKEW_MS) return undefined;
  if (!(await verifyEd25519(key, deviceLinkMessage(target, connId, ts), proof))) return undefined;
  const directory = userDirectoryStub(env);
  if (!directory) return undefined;
  try {
    return (await directory.pluginKeyDevice(key, auth.userId)) === device ? device : undefined;
  } catch (e) {
    console.warn("device link: key check failed", e instanceof Error ? e.name : "error");
    return undefined;
  }
};

/** A `/device/{deviceId}/ws` dial forwarded into the device's room (`d2/`,
 * the same staging→prod identity break as `s2/`), with the calling device
 * stamped when the dial proves it. */
export const dialDeviceRoom = async (env: Env, auth: Verified, request: Request, url: URL, deviceId: string): Promise<Response> => {
  const role = url.searchParams.get("role") === "host" ? "host" : "client";
  const connId = url.searchParams.get("connId") ?? crypto.randomUUID();
  const caller = role === "client" ? await callerDevice(env, auth, url, deviceId, connId) : undefined;
  return forward(
    env.DEVICE_ROOMS,
    `d2/${deviceId}`,
    request,
    auth,
    "/ws",
    `?role=${role}&connId=${encodeURIComponent(connId)}`,
    undefined,
    caller
  );
};
