/**
 * `/vault/{orgId}/…` — the main Worker's HTTP face of the credential vault
 * (docs/design/cloud-device.md, "HTTP contract"). The vault Worker has no
 * routes of its own; this table translates each request into one `VAULT`
 * service-binding RPC carrying the identity WE verified, and maps the
 * vault's `VaultResult` back to an HTTP status + `{error, message}`.
 *
 * Validation here is shape-only (ids, provider names, body types); the vault
 * re-validates everything it acts on. Runner bearers only reach the grant
 * route (runner-policy.ts), and only for their own device.
 *
 * `VAULT_PROVIDERS` is the one value imported from the contract file — it is
 * dependency-free, so no vault code enters this bundle.
 */
import {
  VAULT_PROVIDERS,
  type EnrollDeviceRequest,
  type GrantRequest,
  type PutCredentialRequest,
  type VaultCaller,
  type VaultProviderId,
  type VaultResult
} from "../vault/src/api";
import type { Verified } from "./auth";
import type { Env } from "./env";
import { json, readJsonBody } from "./http";

const ID_RE = /^[A-Za-z0-9_-]{1,128}$/;
const KEY_RE = /^[A-Za-z0-9_-]{1,128}$/;
const MAX_BODY_BYTES = 64 * 1024;
const MAX_AUTHORIZED_DEVICES = 64;

const bad = (message: string): Response => json({ error: "bad_request", message }, 400);

const isProvider = (value: unknown): value is VaultProviderId =>
  typeof value === "string" && (VAULT_PROVIDERS as readonly string[]).includes(value);

const deviceList = (value: unknown): string[] | undefined =>
  Array.isArray(value) &&
  value.length <= MAX_AUTHORIZED_DEVICES &&
  value.every((d) => typeof d === "string" && ID_RE.test(d))
    ? (value as string[])
    : undefined;

const isObject = (value: unknown): value is Record<string, unknown> =>
  typeof value === "object" && value !== null && !Array.isArray(value);

/** `VaultResult` → HTTP. The vault picks the status; anything outside the
 * error range is a contract bug, answered as a bad gateway. */
export const vaultResponse = <T>(result: VaultResult<T>): Response => {
  if (result.ok) return json(result.value);
  const status = result.status >= 400 && result.status <= 599 ? result.status : 502;
  return json({ error: result.error, message: result.message }, status);
};

/** Handle a `/vault/*` route; undefined means "not a vault route". */
export const handleVaultRoute = async (
  request: Request,
  env: Pick<Env, "VAULT">,
  auth: Verified,
  url: URL
): Promise<Response | undefined> => {
  const parts = url.pathname.split("/").filter(Boolean);
  if (parts[0] !== "vault") return undefined;
  const orgId = parts[1];
  if (!orgId || !ID_RE.test(orgId)) return json({ error: "not_found" }, 404);
  if (auth.orgId !== orgId) return json({ error: "forbidden", message: "Wrong organization." }, 403);
  const vault = env.VAULT;
  if (!vault) {
    return json({ error: "unavailable", message: "The credential vault is not configured." }, 503);
  }
  const caller: VaultCaller = {
    userId: auth.userId,
    orgId,
    kind: auth.kind,
    ...(auth.deviceId ? { deviceId: auth.deviceId } : {})
  };
  const method = request.method;
  const rest = parts.slice(2);

  const body = async (): Promise<Record<string, unknown> | Response> => {
    const parsed = await readJsonBody(request, MAX_BODY_BYTES);
    if (parsed === undefined) return bad("Body must be a JSON object.");
    return isObject(parsed) ? parsed : bad("Body must be a JSON object.");
  };

  const call = async <T>(run: () => Promise<VaultResult<T>>): Promise<Response> => {
    try {
      return vaultResponse(await run());
    } catch (e) {
      console.error("vault rpc failed", String(e));
      return json({ error: "unavailable", message: "The credential vault is unavailable." }, 503);
    }
  };

  // GET /vault/{orgId}
  if (rest.length === 0 && method === "GET") return call(() => vault.status(caller));

  // /vault/{orgId}/credentials/{provider}
  if (rest[0] === "credentials" && rest.length === 2) {
    const provider = rest[1];
    if (!isProvider(provider)) return bad("Unknown provider.");
    if (method === "PUT") {
      const b = await body();
      if (b instanceof Response) return b;
      const authorizedDevices = deviceList(b.authorizedDevices);
      if (!isObject(b.material) || !authorizedDevices) {
        return bad("Expected {material, authorizedDevices}.");
      }
      const req = { material: b.material, authorizedDevices } as unknown as PutCredentialRequest;
      return call(() => vault.putCredential(caller, provider, req));
    }
    if (method === "PATCH") {
      const b = await body();
      if (b instanceof Response) return b;
      const authorizedDevices = deviceList(b.authorizedDevices);
      if (!authorizedDevices) return bad("Expected {authorizedDevices}.");
      return call(() => vault.authorize(caller, provider, authorizedDevices));
    }
    if (method === "DELETE") return call(() => vault.disconnect(caller, provider));
  }

  // POST /vault/{orgId}/devices/enroll — laptops enroll their own key here.
  // Cloud devices are enrolled by their CloudAccount DO with the runner key;
  // refusing `cloud` (and the `cloud-` id space) here keeps a user's laptop
  // key from squatting the Cloud device's vault identity.
  if (rest[0] === "devices" && rest[1] === "enroll" && rest.length === 2 && method === "POST") {
    const b = await body();
    if (b instanceof Response) return b;
    const { deviceId, kind, publicKey } = b;
    if (typeof deviceId !== "string" || !ID_RE.test(deviceId) || deviceId.startsWith("cloud-")) {
      return bad("Bad deviceId.");
    }
    if (kind !== "laptop") return bad("Only laptop devices enroll here.");
    if (typeof publicKey !== "string" || !KEY_RE.test(publicKey)) return bad("Bad publicKey.");
    const req: EnrollDeviceRequest = { deviceId, kind, publicKey };
    return call(() => vault.enrollDevice(caller, req));
  }

  // POST /vault/{orgId}/devices/{deviceId}/revoke
  if (rest[0] === "devices" && rest[2] === "revoke" && rest.length === 3 && method === "POST") {
    const deviceId = rest[1]!;
    if (!ID_RE.test(deviceId)) return bad("Bad deviceId.");
    return call(() => vault.revokeDevice(caller, deviceId));
  }

  // POST /vault/{orgId}/disable {disabled}
  if (rest[0] === "disable" && rest.length === 1 && method === "POST") {
    const b = await body();
    if (b instanceof Response) return b;
    if (typeof b.disabled !== "boolean") return bad("Expected {disabled: boolean}.");
    const disabled = b.disabled;
    return call(() => vault.setDisabled(caller, disabled));
  }

  // POST /vault/{orgId}/grant — user or runner
  if (rest[0] === "grant" && rest.length === 1 && method === "POST") {
    const b = await body();
    if (b instanceof Response) return b;
    const { provider, deviceId, ts, sig, repo } = b;
    if (!isProvider(provider)) return bad("Unknown provider.");
    if (typeof deviceId !== "string" || !ID_RE.test(deviceId)) return bad("Bad deviceId.");
    if (typeof ts !== "number" || !Number.isSafeInteger(ts)) return bad("Bad ts.");
    if (typeof sig !== "string" || !KEY_RE.test(sig)) return bad("Bad sig.");
    // `github` grants name the repository (owner/name) the vault mints an
    // installation token for; the vault validates it.
    if (repo !== undefined && (typeof repo !== "string" || repo.length > 200)) return bad("Bad repo.");
    if (auth.kind === "runner" && deviceId !== auth.deviceId) {
      return json({ error: "forbidden", message: "A runner may only request grants for itself." }, 403);
    }
    const req: GrantRequest = { provider, deviceId, ts, sig, ...(repo !== undefined ? { repo } : {}) };
    return call(() => vault.grant(caller, req));
  }

  // GET /vault/{orgId}/github/repos?q= — the repositories the user's GitHub
  // connection can see, for picking a Cloud project. The vault calls GitHub
  // itself; the token never leaves it. Runners are refused by runner-policy.
  if (rest[0] === "github" && rest[1] === "repos" && rest.length === 2 && method === "GET") {
    const query = url.searchParams.get("q") ?? undefined;
    if (query !== undefined && query.length > 200) return bad("Query too long.");
    return call(async () => {
      const result = await vault.githubRepos(caller, query);
      return result.ok ? { ok: true as const, value: { repos: result.value } } : result;
    });
  }

  // GET /vault/{orgId}/github/branches?repo=owner/name — branch names for
  // the branch a new Cloud session starts from. Runners are refused by
  // runner-policy.
  if (rest[0] === "github" && rest[1] === "branches" && rest.length === 2 && method === "GET") {
    const repo = url.searchParams.get("repo");
    if (!repo || repo.length > 200) return bad("Expected ?repo=owner/name.");
    return call(async () => {
      const result = await vault.githubBranches(caller, repo);
      return result.ok ? { ok: true as const, value: { branches: result.value } } : result;
    });
  }

  // GitHub device flow: start, then poll by flow id.
  if (rest[0] === "github" && rest[1] === "device" && method === "POST") {
    if (rest.length === 2) {
      const b = await body();
      if (b instanceof Response) return b;
      const authorizedDevices = deviceList(b.authorizedDevices);
      if (!authorizedDevices) return bad("Expected {authorizedDevices}.");
      return call(() => vault.githubDeviceStart(caller, authorizedDevices));
    }
    if (rest.length === 3) {
      const flowId = rest[2]!;
      if (!ID_RE.test(flowId)) return bad("Bad flowId.");
      return call(() => vault.githubDevicePoll(caller, flowId));
    }
  }

  return json({ error: "not_found" }, 404);
};
