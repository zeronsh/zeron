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
  CONNECTION_ID_RE,
  LEGACY_VAULT_PROVIDERS,
  VAULT_PROVIDERS,
  type ConnectionGrantManyRequest,
  type ConnectionGrantRequest,
  type EnrollDeviceRequest,
  type GrantManyRequest,
  type GrantRequest,
  type PutConnectionRequest,
  type PutCredentialRequest,
  type UpdateConnectionRequest,
  type VaultCaller,
  type VaultConnectionKind,
  type VaultProviderId,
  type VaultResult,
  type VaultStatusView
} from "../vault/src/api";
import type { Verified } from "./auth";
import type { Env } from "./env";
import { json, readJsonBody } from "./http";
import { cloudAccountName } from "./cloud/policy";

const ID_RE = /^[A-Za-z0-9_-]{1,128}$/;
const KEY_RE = /^[A-Za-z0-9_-]{1,128}$/;
const MAX_BODY_BYTES = 64 * 1024;
const MAX_AUTHORIZED_DEVICES = 64;
const MAX_CONNECTION_BATCH = 100;

const bad = (message: string): Response => json({ error: "bad_request", message }, 400);

const isProvider = (value: unknown): value is VaultProviderId =>
  typeof value === "string" && (VAULT_PROVIDERS as readonly string[]).includes(value);

const deviceList = (value: unknown): string[] | undefined =>
  Array.isArray(value) &&
  value.length <= MAX_AUTHORIZED_DEVICES &&
  value.every((d) => typeof d === "string" && ID_RE.test(d))
    ? (value as string[])
    : undefined;

const connectionDeviceList = (value: unknown): string[] | undefined =>
  Array.isArray(value) &&
  value.length <= MAX_AUTHORIZED_DEVICES &&
  value.every((d) => typeof d === "string" && (d === "*" || ID_RE.test(d)))
    ? (value as string[])
    : undefined;

const decodeConnectionId = (raw: string): string | undefined => {
  try {
    const decoded = decodeURIComponent(raw);
    return CONNECTION_ID_RE.test(decoded) ? decoded : undefined;
  } catch {
    return undefined;
  }
};

const isObject = (value: unknown): value is Record<string, unknown> =>
  typeof value === "object" && value !== null && !Array.isArray(value);

/** A login's display identity: short strings only. `null` = malformed. */
const accountProfile = (value: unknown): Record<string, string> | null => {
  if (!isObject(value)) return null;
  const out: Record<string, string> = {};
  for (const [key, field] of Object.entries(value)) {
    if (!["email", "displayName", "organization", "plan"].includes(key)) continue;
    if (field === undefined || field === null) continue;
    if (typeof field !== "string" || field.length > 200) return null;
    out[key] = field;
  }
  return out;
};

/** Header naming the providers a client understands (crates/proto
 * `VAULT_PROVIDERS_HEADER`). A status lists only those: an older client reads
 * provider names strictly and would refuse the whole status over one it
 * doesn't know. Without the header, the providers every client knows. */
export const VAULT_PROVIDERS_HEADER = "x-zeron-vault-providers";

export const understoodProviders = (request: Request): ReadonlySet<string> => {
  const header = request.headers.get(VAULT_PROVIDERS_HEADER);
  if (header === null) return new Set(LEGACY_VAULT_PROVIDERS);
  return new Set(
    header
      .split(",")
      .map((name) => name.trim())
      .filter(Boolean)
  );
};

/** A status as `understood` can read it. Statuses are what every vault call
 * that changes something answers, so all of them pass through here. Only
 * `VaultStatusView` (`available: true` + `devices` + `connections`) is filtered,
 * leaving `VaultConnectionsView` untouched. */
const forClient = <T>(result: VaultResult<T>, understood: ReadonlySet<string>): VaultResult<T> => {
  if (
    !result.ok ||
    !isObject(result.value) ||
    result.value.available !== true ||
    !Array.isArray(result.value.devices) ||
    !Array.isArray(result.value.connections)
  ) {
    return result;
  }
  const status = result.value as unknown as VaultStatusView;
  return {
    ok: true,
    value: { ...status, connections: status.connections.filter((c) => understood.has(c.provider)) } as unknown as T
  };
};

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
  env: Pick<Env, "VAULT" | "CLOUD_ACCOUNTS">,
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

  const understood = understoodProviders(request);
  const call = async <T>(run: () => Promise<VaultResult<T>>): Promise<Response> => {
    try {
      return vaultResponse(forClient(await run(), understood));
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
        return bad("Expected {material, authorizedDevices, profile?}.");
      }
      const profile = b.profile === undefined ? undefined : accountProfile(b.profile);
      if (profile === null) return bad("profile must be {email?, displayName?, organization?, plan?} strings.");
      const req = {
        material: b.material,
        authorizedDevices,
        ...(profile ? { profile } : {})
      } as unknown as PutCredentialRequest;
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

  // /vault/{orgId}/accounts/{provider}[/{slot}[/usage]] — a provider's
  // stored logins: list, pick the active one, forget one, read one's usage.
  if (rest[0] === "accounts" && rest.length >= 2) {
    const provider = rest[1];
    if (!isProvider(provider)) return bad("Unknown provider.");
    const list = <T>(run: () => Promise<VaultResult<T>>) =>
      call(async () => {
        const result = await run();
        return result.ok ? { ok: true as const, value: { accounts: result.value } } : result;
      });
    if (rest.length === 2 && method === "GET") return list(() => vault.accounts(caller, provider));
    if (rest.length === 3 && rest[2] === "active" && method === "POST") {
      const b = await body();
      if (b instanceof Response) return b;
      const slot = b.slot;
      if (slot !== null && (typeof slot !== "string" || !ID_RE.test(slot))) return bad("Expected {slot: id | null}.");
      return list(() => vault.activateAccount(caller, provider, slot));
    }
    const slot = rest[2];
    if (slot === undefined || !ID_RE.test(slot)) return bad("Bad account id.");
    if (rest.length === 3 && method === "DELETE") return list(() => vault.forgetAccount(caller, provider, slot));
    if (rest.length === 4 && rest[3] === "usage" && method === "GET") {
      return call(() => vault.accountUsage(caller, provider, slot));
    }
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
    // A Cloud machine's GitHub access is its own chat's repositories (its
    // one, or each of a project of several), nothing else the user's GitHub
    // reaches. A delegate's first grant takes the GitHub login of the
    // session that started it; only this Worker says so, never a request's
    // body.
    let inheritFrom: string | undefined;
    if (auth.kind === "runner" && provider === "github") {
      const cloud =
        env.CLOUD_ACCOUNTS && auth.orgId
          ? env.CLOUD_ACCOUNTS.get(env.CLOUD_ACCOUNTS.idFromName(cloudAccountName(auth.orgId, auth.userId)))
          : undefined;
      const owner = { orgId: auth.orgId ?? "", userId: auth.userId };
      const own = cloud ? await cloud.reposOfDevice(owner, deviceId) : [];
      if (typeof repo !== "string" || !own.some((name) => name.toLowerCase() === repo.toLowerCase())) {
        return json({ error: "forbidden", message: "A Cloud machine may only get GitHub access to its own chat's repositories." }, 403);
      }
      inheritFrom = await cloud?.parentDeviceOf(owner, deviceId);
    }
    const req: GrantRequest = {
      provider,
      deviceId,
      ts,
      sig,
      ...(repo !== undefined ? { repo } : {}),
      ...(inheritFrom !== undefined ? { inheritFrom } : {})
    };
    return call(() => vault.grant(caller, req));
  }

  // POST /vault/{orgId}/grants — several providers' grants at once (user or
  // runner). Never `github`: a Cloud machine's GitHub grant is checked
  // against its own chat's repositories above, one at a time.
  if (rest[0] === "grants" && rest.length === 1 && method === "POST") {
    const b = await body();
    if (b instanceof Response) return b;
    const { providers, deviceId, ts, sig } = b;
    if (
      !Array.isArray(providers) ||
      providers.length === 0 ||
      providers.length > VAULT_PROVIDERS.length ||
      !providers.every((p) => isProvider(p) && p !== "github")
    ) {
      return bad("Expected providers: a list of providers other than github.");
    }
    if (typeof deviceId !== "string" || !ID_RE.test(deviceId)) return bad("Bad deviceId.");
    if (typeof ts !== "number" || !Number.isSafeInteger(ts)) return bad("Bad ts.");
    if (typeof sig !== "string" || !KEY_RE.test(sig)) return bad("Bad sig.");
    if (auth.kind === "runner" && deviceId !== auth.deviceId) {
      return json({ error: "forbidden", message: "A runner may only request grants for itself." }, 403);
    }
    const req: GrantManyRequest = { providers: providers as VaultProviderId[], deviceId, ts, sig };
    return call(() => vault.grantMany(caller, req));
  }

  // GET /vault/{orgId}/github/repos?q= — the repositories the user's GitHub
  // logins can see, merged, for picking a Cloud project; beside them the App
  // installations they come through and any login that didn't answer. The
  // vault calls GitHub itself; the tokens never leave it. Runners are refused
  // by runner-policy.
  if (rest[0] === "github" && rest[1] === "repos" && rest.length === 2 && method === "GET") {
    const query = url.searchParams.get("q") ?? undefined;
    if (query !== undefined && query.length > 200) return bad("Query too long.");
    return call(() => vault.githubListing(caller, query));
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

  // /vault/{orgId}/connections — remote MCP & plugin connections.
  if (rest[0] === "connections") {
    // GET /vault/{orgId}/connections[?kind=mcp|plugin] (user or runner)
    if (rest.length === 1 && method === "GET") {
      const rawKind = url.searchParams.get("kind");
      if (rawKind !== null && rawKind !== "mcp" && rawKind !== "plugin") {
        return bad("kind must be mcp or plugin.");
      }
      const kind: VaultConnectionKind | undefined = rawKind ?? undefined;
      return call(() => vault.listConnections(caller, kind));
    }

    // POST /vault/{orgId}/connections/grant (user or runner)
    if (rest.length === 2 && rest[1] === "grant" && method === "POST") {
      const b = await body();
      if (b instanceof Response) return b;
      const { id, deviceId, ts, sig, forceRefresh } = b;
      if (typeof id !== "string" || !CONNECTION_ID_RE.test(id)) return bad("Bad connection id.");
      if (typeof deviceId !== "string" || !ID_RE.test(deviceId)) return bad("Bad deviceId.");
      if (typeof ts !== "number" || !Number.isSafeInteger(ts)) return bad("Bad ts.");
      if (typeof sig !== "string" || !KEY_RE.test(sig)) return bad("Bad sig.");
      if (forceRefresh !== undefined && typeof forceRefresh !== "boolean") return bad("Bad forceRefresh.");
      if (auth.kind === "runner" && deviceId !== auth.deviceId) {
        return json({ error: "forbidden", message: "A runner may only request grants for itself." }, 403);
      }
      const req: ConnectionGrantRequest = {
        id,
        deviceId,
        ts,
        sig,
        ...(forceRefresh !== undefined ? { forceRefresh } : {})
      };
      return call(() => vault.grantConnection(caller, req));
    }

    // POST /vault/{orgId}/connections/grants (user or runner)
    if (rest.length === 2 && rest[1] === "grants" && method === "POST") {
      const b = await body();
      if (b instanceof Response) return b;
      const { ids, deviceId, ts, sig } = b;
      if (
        !Array.isArray(ids) ||
        ids.length === 0 ||
        ids.length > MAX_CONNECTION_BATCH ||
        !ids.every((item) => typeof item === "string" && CONNECTION_ID_RE.test(item))
      ) {
        return bad("Expected ids: a non-empty list of connection ids.");
      }
      if (typeof deviceId !== "string" || !ID_RE.test(deviceId)) return bad("Bad deviceId.");
      if (typeof ts !== "number" || !Number.isSafeInteger(ts)) return bad("Bad ts.");
      if (typeof sig !== "string" || !KEY_RE.test(sig)) return bad("Bad sig.");
      if (auth.kind === "runner" && deviceId !== auth.deviceId) {
        return json({ error: "forbidden", message: "A runner may only request grants for itself." }, 403);
      }
      const req: ConnectionGrantManyRequest = { ids: ids as string[], deviceId, ts, sig };
      return call(() => vault.grantConnections(caller, req));
    }

    // /vault/{orgId}/connections/{id}[/disconnect] (users only; runners blocked by runner-policy)
    const id = rest[1] !== undefined ? decodeConnectionId(rest[1]) : undefined;
    if (rest.length === 2 && (method === "PUT" || method === "PATCH" || method === "DELETE")) {
      if (!id) return bad("Bad connection id.");
      if (method === "PUT") {
        const b = await body();
        if (b instanceof Response) return b;
        if (typeof b.name !== "string" || typeof b.url !== "string") {
          return bad("Expected {name, url, authKind, ...}.");
        }
        if (b.authKind !== "none" && b.authKind !== "headers" && b.authKind !== "oauth") {
          return bad("authKind must be none, headers, or oauth.");
        }
        if (b.transport !== undefined && b.transport !== "http" && b.transport !== "sse") {
          return bad("transport must be http or sse.");
        }
        if (b.enabled !== undefined && typeof b.enabled !== "boolean") return bad("enabled must be a boolean.");
        if (b.preset !== undefined && typeof b.preset !== "string") return bad("preset must be a string.");
        if (b.accountLabel !== undefined && typeof b.accountLabel !== "string") {
          return bad("accountLabel must be a string.");
        }
        if (b.material !== undefined && !isObject(b.material)) return bad("material must be an object.");
        let authorizedDevices: string[] | undefined;
        if (b.authorizedDevices !== undefined) {
          authorizedDevices = connectionDeviceList(b.authorizedDevices);
          if (!authorizedDevices) return bad("authorizedDevices must be a list of device ids or '*'.");
        }
        let scopes: string[] | undefined;
        if (b.scopes !== undefined) {
          if (!Array.isArray(b.scopes) || !b.scopes.every((s) => typeof s === "string")) {
            return bad("scopes must be a list of strings.");
          }
          scopes = b.scopes as string[];
        }
        const req: PutConnectionRequest = {
          name: b.name,
          url: b.url,
          authKind: b.authKind,
          ...(b.preset !== undefined ? { preset: b.preset } : {}),
          ...(b.transport !== undefined ? { transport: b.transport } : {}),
          ...(b.enabled !== undefined ? { enabled: b.enabled } : {}),
          ...(b.material !== undefined ? { material: b.material as PutConnectionRequest["material"] } : {}),
          ...(authorizedDevices !== undefined ? { authorizedDevices } : {}),
          ...(scopes !== undefined ? { scopes } : {}),
          ...(b.accountLabel !== undefined ? { accountLabel: b.accountLabel } : {})
        };
        return call(() => vault.putConnection(caller, id, req));
      }
      if (method === "PATCH") {
        const b = await body();
        if (b instanceof Response) return b;
        if (b.name !== undefined && typeof b.name !== "string") return bad("name must be a string.");
        if (b.url !== undefined && typeof b.url !== "string") return bad("url must be a string.");
        if (b.transport !== undefined && b.transport !== "http" && b.transport !== "sse") {
          return bad("transport must be http or sse.");
        }
        if (b.enabled !== undefined && typeof b.enabled !== "boolean") return bad("enabled must be a boolean.");
        if (b.accountLabel !== undefined && b.accountLabel !== null && typeof b.accountLabel !== "string") {
          return bad("accountLabel must be a string or null.");
        }
        let authorizedDevices: string[] | undefined;
        if (b.authorizedDevices !== undefined) {
          authorizedDevices = connectionDeviceList(b.authorizedDevices);
          if (!authorizedDevices) return bad("authorizedDevices must be a list of device ids or '*'.");
        }
        const patch: UpdateConnectionRequest = {
          ...(b.name !== undefined ? { name: b.name } : {}),
          ...(b.url !== undefined ? { url: b.url } : {}),
          ...(b.transport !== undefined ? { transport: b.transport } : {}),
          ...(b.enabled !== undefined ? { enabled: b.enabled } : {}),
          ...(authorizedDevices !== undefined ? { authorizedDevices } : {}),
          ...(b.accountLabel !== undefined ? { accountLabel: b.accountLabel } : {})
        };
        return call(() => vault.updateConnection(caller, id, patch));
      }
      return call(() => vault.deleteConnection(caller, id));
    }

    if (rest.length === 3 && rest[2] === "disconnect" && method === "POST") {
      if (!id) return bad("Bad connection id.");
      return call(() => vault.disconnectConnectionAuth(caller, id));
    }
  }

  return json({ error: "not_found" }, 404);
};
