import { env, exports } from "cloudflare:workers";
import { runInDurableObject } from "cloudflare:test";
import { afterEach, describe, expect, it, vi } from "vitest";
import type { ConnectionGrantManyRequest, ConnectionGrantRequest, VaultCaller } from "../../src/api";
import { connectionAad } from "../../src/connections";
import { envelopeKid, loadKeyring, open, sealJson } from "../../src/crypto";
import { connectionsStub, devicesStub } from "../../src/names";
import { makeDevice, type TestDevice } from "../support";
import { enroll, json, mockUpstream, nextTs, runnerCaller, setup, value, vault } from "./helpers";

afterEach(() => vi.restoreAllMocks());

const signedConnGrant = async (
  device: TestDevice,
  userId: string,
  id: string,
  ts = nextTs(),
  forceRefresh?: boolean
): Promise<ConnectionGrantRequest> => ({
  id,
  deviceId: device.deviceId,
  ts,
  sig: await device.grantSig(userId, `conn:${id}`, ts),
  ...(forceRefresh !== undefined ? { forceRefresh } : {})
});

const signedConnGrantMany = async (
  device: TestDevice,
  userId: string,
  ids: readonly string[],
  ts = nextTs()
): Promise<ConnectionGrantManyRequest> => ({
  ids,
  deviceId: device.deviceId,
  ts,
  sig: await device.grantSig(userId, `conns:${ids.join(",")}`, ts)
});

const enrollChild = async (caller: VaultCaller, parentId: string) => {
  const child = await makeDevice(`session-${crypto.randomUUID()}`);
  value(
    await vault().enrollDevice(caller, { deviceId: child.deviceId, kind: "cloud", publicKey: child.publicKey, parentId })
  );
  return child;
};

const dumpStorage = (stub: DurableObjectStub) =>
  runInDurableObject(stub, (_instance, state) => {
    const tables = state.storage.sql
      .exec<{ name: string }>(
        "SELECT name FROM sqlite_master WHERE type = 'table' AND name NOT LIKE '\\_%' ESCAPE '\\' AND name NOT LIKE 'sqlite_%'"
      )
      .toArray();
    return JSON.stringify(tables.map(({ name }) => state.storage.sql.exec(`SELECT * FROM "${name}"`).toArray()));
  });

const storedConnection = (userId: string, id: string) =>
  runInDurableObject(connectionsStub(env, userId), (_instance, state) =>
    state.storage.sql
      .exec<{ generation: number; envelope: string | null; status: string }>(
        "SELECT generation, envelope, status FROM connections WHERE id = ?",
        id
      )
      .toArray()[0]
  );

const replaceEnvelope = (userId: string, id: string, envelope: string) =>
  runInDurableObject(connectionsStub(env, userId), (_instance, state) => {
    state.storage.sql.exec("UPDATE connections SET envelope = ? WHERE id = ?", envelope, id);
  });

describe("VaultConnections CRUD, views & encryption", () => {
  it("supports CRUD, kind filtering, partial updates, disconnectAuth, delete, and never leaks secrets", async () => {
    const { userId, caller, device } = await setup();

    // 1. Public/no-auth MCP connection
    const noAuth = value(
      await vault().putConnection(caller, "mcp:docs", {
        name: "Public Docs",
        url: "https://docs.example.com/mcp",
        authKind: "none"
      })
    );
    expect(noAuth).toMatchObject({
      id: "mcp:docs",
      kind: "mcp",
      name: "Public Docs",
      url: "https://docs.example.com/mcp",
      transport: "http",
      enabled: true,
      authKind: "none",
      status: "connected",
      headerKeys: [],
      scopes: [],
      authorizedDevices: ["*"]
    });

    // 2. Headers-auth plugin connection
    const headerSecret = "Bearer SECRET_HEADER_VALUE_9999";
    const apiKeySecret = "x-api-SECRET_KEY_8888";
    const plugin = value(
      await vault().putConnection(caller, "plugin:linear", {
        name: "Linear",
        preset: "linear",
        url: "https://mcp.linear.app/sse",
        transport: "sse",
        authKind: "headers",
        accountLabel: "eng@acme.com",
        material: {
          kind: "headers",
          headers: {
            Authorization: headerSecret,
            "X-API-Key": apiKeySecret
          }
        }
      })
    );
    expect(plugin).toMatchObject({
      id: "plugin:linear",
      kind: "plugin",
      name: "Linear",
      preset: "linear",
      transport: "sse",
      authKind: "headers",
      status: "connected",
      headerKeys: ["Authorization", "X-API-Key"],
      accountLabel: "eng@acme.com",
      authorizedDevices: ["*"]
    });

    // 3. OAuth connection without material -> needs_auth
    const pending = value(
      await vault().putConnection(caller, "mcp:github", {
        name: "GitHub MCP",
        url: "https://api.githubcopilot.com/mcp/",
        authKind: "oauth",
        scopes: ["repo", "read:user"]
      })
    );
    expect(pending.status).toBe("needs_auth");
    expect(pending.scopes).toEqual(["repo", "read:user"]);

    // List all & filter by kind
    const all = value(await vault().listConnections(caller));
    expect(all.disabled).toBe(false);
    expect(all.connections.map((c) => c.id).sort()).toEqual(["mcp:docs", "mcp:github", "plugin:linear"]);

    const onlyMcp = value(await vault().listConnections(caller, "mcp"));
    expect(onlyMcp.connections.map((c) => c.id).sort()).toEqual(["mcp:docs", "mcp:github"]);

    const onlyPlugin = value(await vault().listConnections(caller, "plugin"));
    expect(onlyPlugin.connections.map((c) => c.id)).toEqual(["plugin:linear"]);

    // Partial update preserves sealed material and clears accountLabel when null
    const updated = value(
      await vault().updateConnection(caller, "plugin:linear", {
        name: "Linear Prod",
        accountLabel: null,
        authorizedDevices: [device.deviceId]
      })
    );
    expect(updated.name).toBe("Linear Prod");
    expect(updated.accountLabel).toBeUndefined();
    expect(updated.authorizedDevices).toEqual([device.deviceId]);
    expect(updated.headerKeys).toEqual(["Authorization", "X-API-Key"]);

    // Grant works after update with original sealed headers intact
    const grant = value(await vault().grantConnection(caller, await signedConnGrant(device, userId, "plugin:linear")));
    expect(grant).toEqual({
      id: "plugin:linear",
      url: "https://mcp.linear.app/sse",
      transport: "sse",
      headers: { Authorization: headerSecret, "X-API-Key": apiKeySecret },
      generation: 2
    });

    // No-auth grant returns empty headers
    const docsGrant = value(await vault().grantConnection(caller, await signedConnGrant(device, userId, "mcp:docs")));
    expect(docsGrant).toEqual({
      id: "mcp:docs",
      url: "https://docs.example.com/mcp",
      transport: "http",
      headers: {},
      generation: 1
    });

    // Secrets never appear in views, audit log, or SQLite storage
    const audit = value(await devicesStub(env, userId).auditLog(userId));
    const surfaces: Record<string, string> = {
      list: JSON.stringify(all),
      viaEntrypoint: JSON.stringify(await exports.VaultApi.listConnections(caller)),
      audit: JSON.stringify(audit),
      connectionsStorage: await dumpStorage(connectionsStub(env, userId)),
      devicesStorage: await dumpStorage(devicesStub(env, userId))
    };
    for (const [surface, text] of Object.entries(surfaces)) {
      expect(text.includes(headerSecret), `${surface} leaked headerSecret`).toBe(false);
      expect(text.includes(apiKeySecret), `${surface} leaked apiKeySecret`).toBe(false);
    }

    // disconnectConnectionAuth wipes envelope, sets needs_auth, keeps config
    const disconnected = value(await vault().disconnectConnectionAuth(caller, "plugin:linear"));
    expect(disconnected).toMatchObject({
      id: "plugin:linear",
      name: "Linear Prod",
      status: "needs_auth",
      headerKeys: []
    });
    expect((await storedConnection(userId, "plugin:linear"))?.envelope).toBeNull();
    expect(await vault().grantConnection(caller, await signedConnGrant(device, userId, "plugin:linear"))).toMatchObject({
      ok: false,
      error: "needs_reconnect",
      status: 409
    });

    // deleteConnection removes row idempotently; recreating bumps generation (never repeats AAD)
    expect(value(await vault().deleteConnection(caller, "plugin:linear"))).toEqual({ deleted: true });
    expect(value(await vault().deleteConnection(caller, "plugin:linear"))).toEqual({ deleted: false });
    expect(await vault().grantConnection(caller, await signedConnGrant(device, userId, "plugin:linear"))).toMatchObject({
      ok: false,
      error: "not_found",
      status: 404
    });

    value(
      await vault().putConnection(caller, "plugin:linear", {
        name: "Linear Recreated",
        url: "https://mcp.linear.app/sse",
        authKind: "none"
      })
    );
    expect((await storedConnection(userId, "plugin:linear"))?.generation).toBeGreaterThan(3);
  });

  it("lazily re-wraps envelopes sealed under VAULT_KEK_PREVIOUS and enforces AAD binding", async () => {
    const { userId, caller, device } = await setup();
    const secretHeaders = { kind: "headers" as const, headers: { Authorization: "Bearer rotate-me" } };
    value(
      await vault().putConnection(caller, "mcp:rotate", {
        name: "Rotate",
        url: "https://rotate.example.com/mcp",
        authKind: "headers",
        material: secretHeaders
      })
    );

    const before = (await storedConnection(userId, "mcp:rotate"))!;
    const current = await loadKeyring({ VAULT_KEK: env.VAULT_KEK });
    const previous = await loadKeyring({ VAULT_KEK: env.VAULT_KEK_PREVIOUS });
    expect(envelopeKid(before.envelope!)).toBe(current.current.kid);

    const aad = connectionAad(userId, "mcp:rotate", before.generation);
    await replaceEnvelope(userId, "mcp:rotate", await sealJson(previous, secretHeaders, aad));
    expect(envelopeKid((await storedConnection(userId, "mcp:rotate"))!.envelope!)).toBe(previous.current.kid);

    const grant = value(await vault().grantConnection(caller, await signedConnGrant(device, userId, "mcp:rotate")));
    expect(grant.headers).toEqual({ Authorization: "Bearer rotate-me" });
    expect(grant.generation).toBe(before.generation);

    // Persisted re-wrapped under current KEK with same generation & AAD
    const after = (await storedConnection(userId, "mcp:rotate"))!;
    expect(after.generation).toBe(before.generation);
    expect(envelopeKid(after.envelope!)).toBe(current.current.kid);
    expect(await open(current, after.envelope!, aad)).toMatchObject({ rewrap: false });

    // Copying Alice's envelope into Bob's DO fails AAD verification
    const bob = await setup();
    value(
      await vault().putConnection(bob.caller, "mcp:rotate", {
        name: "Bob Rotate",
        url: "https://rotate.example.com/mcp",
        authKind: "headers",
        material: { kind: "headers", headers: { Authorization: "Bearer bob" } }
      })
    );
    await replaceEnvelope(bob.userId, "mcp:rotate", after.envelope!);
    expect(await vault().grantConnection(bob.caller, await signedConnGrant(bob.device, bob.userId, "mcp:rotate"))).toMatchObject({
      ok: false,
      error: "unavailable",
      status: 500
    });
  });

  it("rejects invalid connection ids", async () => {
    const { caller } = await setup();
    for (const badId of ["", "mcp:", "other:foo", "mcp:UPPER", "mcp:-leading", "mcp:has space", "mcp:" + "a".repeat(65)]) {
      expect(
        await vault().putConnection(caller, badId, { name: "Bad", url: "https://example.com/mcp", authKind: "none" }),
        badId
      ).toMatchObject({ ok: false, error: "bad_request", status: 400 });
    }
  });
});

describe("VaultConnections grants, device authorization, runners & kill switches", () => {
  it("enforces wildcard '*', explicit laptop, and Cloud parent/child authorization plus batch grants", async () => {
    const { userId, caller, device: laptop } = await setup("laptop");
    const otherLaptop = await enroll(caller, "laptop");
    const cloudParentId = `cloud-${crypto.randomUUID()}`;
    const cloudChild = await enrollChild(caller, cloudParentId);
    const runner = runnerCaller(userId, cloudChild.deviceId);

    value(
      await vault().putConnection(caller, "mcp:all", {
        name: "All Devices",
        url: "https://all.example.com/mcp",
        authKind: "headers",
        material: { kind: "headers", headers: { Authorization: "Bearer all" } }
      })
    );
    value(
      await vault().putConnection(caller, "mcp:laptop-only", {
        name: "Laptop Only",
        url: "https://laptop.example.com/mcp",
        authKind: "headers",
        authorizedDevices: [laptop.deviceId],
        material: { kind: "headers", headers: { Authorization: "Bearer laptop" } }
      })
    );
    value(
      await vault().putConnection(caller, "mcp:cloud-only", {
        name: "Cloud Only",
        url: "https://cloud.example.com/mcp",
        authKind: "headers",
        authorizedDevices: [cloudParentId],
        material: { kind: "headers", headers: { Authorization: "Bearer cloud" } }
      })
    );
    value(
      await vault().putConnection(caller, "mcp:disabled-conn", {
        name: "Disabled",
        url: "https://disabled.example.com/mcp",
        authKind: "none",
        enabled: false
      })
    );

    // Runner listConnections filters to enabled + authorized (wildcard or parent match)
    const runnerList = value(await vault().listConnections(runner));
    expect(runnerList.disabled).toBe(false);
    expect(runnerList.connections.map((c) => c.id).sort()).toEqual(["mcp:all", "mcp:cloud-only"]);

    // Runner cannot mutate connections
    expect(
      await vault().putConnection(runner, "mcp:evil", { name: "Evil", url: "https://evil.example.com/mcp", authKind: "none" })
    ).toMatchObject({ ok: false, error: "forbidden", status: 403 });
    expect(await vault().updateConnection(runner, "mcp:all", { enabled: false })).toMatchObject({
      ok: false,
      error: "forbidden",
      status: 403
    });
    expect(await vault().deleteConnection(runner, "mcp:all")).toMatchObject({ ok: false, error: "forbidden", status: 403 });
    expect(await vault().disconnectConnectionAuth(runner, "mcp:all")).toMatchObject({
      ok: false,
      error: "forbidden",
      status: 403
    });

    // Runner cannot draw grant for another device even with valid signature
    expect(
      await vault().grantConnection(runner, await signedConnGrant(laptop, userId, "mcp:laptop-only"))
    ).toMatchObject({ ok: false, error: "forbidden", status: 403 });

    // Cloud child authorized via parent gets mcp:cloud-only via real VaultApi RPC
    const cloudGrant = value(
      await exports.VaultApi.grantConnection(runner, await signedConnGrant(cloudChild, userId, "mcp:cloud-only"))
    );
    expect(cloudGrant.headers).toEqual({ Authorization: "Bearer cloud" });

    // Batch grant returns authorized grants and reports missing/unauthorized/disabled
    const batch = value(
      await vault().grantConnections(
        runner,
        await signedConnGrantMany(cloudChild, userId, [
          "mcp:all",
          "mcp:cloud-only",
          "mcp:laptop-only",
          "mcp:disabled-conn",
          "mcp:nonexistent"
        ])
      )
    );
    expect(batch.grants.map((g) => g.id).sort()).toEqual(["mcp:all", "mcp:cloud-only"]);
    expect(batch.missing).toEqual(
      expect.arrayContaining([
        expect.objectContaining({ id: "mcp:laptop-only", error: "not_authorized" }),
        expect.objectContaining({ id: "mcp:disabled-conn", error: "not_authorized" }),
        expect.objectContaining({ id: "mcp:nonexistent", error: "not_found" })
      ])
    );

    // Other laptop gets wildcard, denied laptop-only & cloud-only
    value(await vault().grantConnection(caller, await signedConnGrant(otherLaptop, userId, "mcp:all")));
    expect(await vault().grantConnection(caller, await signedConnGrant(otherLaptop, userId, "mcp:laptop-only"))).toMatchObject({
      ok: false,
      error: "not_authorized",
      status: 403
    });
    expect(await vault().grantConnection(caller, await signedConnGrant(otherLaptop, userId, "mcp:cloud-only"))).toMatchObject({
      ok: false,
      error: "not_authorized",
      status: 403
    });
  });

  it("blocks grants and runner listing on device/parent revocation and kill switches, while allowing cleanup", async () => {
    const { userId, caller, device: laptop } = await setup("laptop");
    const cloudParentId = `cloud-${crypto.randomUUID()}`;
    const cloudChild = await enrollChild(caller, cloudParentId);
    const runner = runnerCaller(userId, cloudChild.deviceId);

    value(
      await vault().putConnection(caller, "mcp:one", {
        name: "One",
        url: "https://one.example.com/mcp",
        authKind: "headers",
        material: { kind: "headers", headers: { Authorization: "Bearer one" } }
      })
    );
    value(
      await vault().putConnection(caller, "mcp:two", {
        name: "Two",
        url: "https://two.example.com/mcp",
        authKind: "headers",
        material: { kind: "headers", headers: { Authorization: "Bearer two" } }
      })
    );

    // Revoking parent blocks child grants AND runner listConnections
    value(await vault().revokeDevice(caller, cloudParentId));
    expect(await vault().grantConnection(runner, await signedConnGrant(cloudChild, userId, "mcp:one"))).toMatchObject({
      ok: false,
      error: "device_revoked",
      status: 403
    });
    expect(await vault().listConnections(runner)).toMatchObject({
      ok: false,
      error: "device_revoked",
      status: 403
    });

    // Global kill switch -> 503 for everything
    const globalOff = vault({ VAULT_DISABLED: "1" });
    expect(await globalOff.listConnections(caller)).toMatchObject({ ok: false, error: "disabled", status: 503 });
    expect(await globalOff.grantConnection(caller, await signedConnGrant(laptop, userId, "mcp:one"))).toMatchObject({
      ok: false,
      error: "disabled",
      status: 503
    });

    // Per-user kill switch blocks grants, writes, and runner listing
    value(await vault().setDisabled(caller, true));
    expect(await vault().grantConnection(caller, await signedConnGrant(laptop, userId, "mcp:one"))).toMatchObject({
      ok: false,
      error: "disabled",
      status: 403
    });
    expect(
      await vault().putConnection(caller, "mcp:three", { name: "Three", url: "https://three.example.com/mcp", authKind: "none" })
    ).toMatchObject({ ok: false, error: "disabled", status: 403 });
    expect(await vault().updateConnection(caller, "mcp:one", { name: "Renamed" })).toMatchObject({
      ok: false,
      error: "disabled",
      status: 403
    });
    expect(await vault().listConnections(runner)).toMatchObject({ ok: false, error: "disabled", status: 403 });

    // User listConnections reports disabled: true; disconnectConnectionAuth & deleteConnection still work!
    const whileDisabled = value(await vault().listConnections(caller));
    expect(whileDisabled.disabled).toBe(true);
    expect(whileDisabled.connections).toHaveLength(2);
    expect(value(await vault().disconnectConnectionAuth(caller, "mcp:one")).status).toBe("needs_auth");
    expect(value(await vault().deleteConnection(caller, "mcp:two"))).toEqual({ deleted: true });
  });
});

describe("VaultConnections OAuth single-flight refresh & SSRF defense", () => {
  const TOKEN_URL = "https://auth.example.com/oauth/token";

  it("does not refresh when token has > 5 min left; shares ONE refresh across concurrent grants with rotating refresh_token", async () => {
    const { userId, caller, device: laptop } = await setup("laptop");
    const cloud = await enroll(caller, "cloud");

    // 1. Fresh token (1 hour left) -> no refresh
    value(
      await vault().putConnection(caller, "mcp:oauth", {
        name: "OAuth MCP",
        url: "https://mcp.example.com/v1",
        authKind: "oauth",
        material: {
          kind: "oauth",
          accessToken: "at-initial-SECRET",
          refreshToken: "rt-1-SECRET",
          tokenType: "Bearer",
          expiresAt: Date.now() + 60 * 60_000,
          tokenEndpoint: TOKEN_URL,
          clientId: "cid-1",
          clientSecret: "csec-1-SECRET",
          scopes: ["read", "write"],
          resource: "https://mcp.example.com/v1"
        }
      })
    );

    const noCalls = mockUpstream({});
    const freshGrant = value(await vault().grantConnection(caller, await signedConnGrant(laptop, userId, "mcp:oauth")));
    expect(freshGrant).toMatchObject({
      id: "mcp:oauth",
      headers: { Authorization: "Bearer at-initial-SECRET" },
      generation: 1
    });
    // Grant must NEVER expose refreshToken or clientSecret
    expect(JSON.stringify(freshGrant)).not.toContain("rt-1-SECRET");
    expect(JSON.stringify(freshGrant)).not.toContain("csec-1-SECRET");
    expect(noCalls.calls).toHaveLength(0);
    vi.restoreAllMocks();

    // 2. Replace with expiring token (2 min left, within 5 min window) -> single-flight concurrent refresh
    value(
      await vault().putConnection(caller, "mcp:oauth", {
        name: "OAuth MCP",
        url: "https://mcp.example.com/v1",
        authKind: "oauth",
        material: {
          kind: "oauth",
          accessToken: "at-expiring",
          refreshToken: "rt-1",
          tokenType: "Bearer",
          expiresAt: Date.now() + 2 * 60_000,
          tokenEndpoint: TOKEN_URL,
          clientId: "cid-1",
          clientSecret: "csec-1",
          scopes: ["read", "write"],
          resource: "https://mcp.example.com/v1"
        }
      })
    );

    let release!: () => void;
    const held = new Promise<void>((resolve) => (release = resolve));
    const upstream = mockUpstream({
      [TOKEN_URL]: async () => {
        await held;
        return json({
          access_token: "at-refreshed-1",
          refresh_token: "rt-2-rotated",
          token_type: "Bearer",
          expires_in: 3600
        });
      }
    });

    const first = vault().grantConnection(caller, await signedConnGrant(laptop, userId, "mcp:oauth"));
    await vi.waitFor(() => expect(upstream.count(TOKEN_URL)).toBe(1));

    let secondSettled = false;
    const second = vault()
      .grantConnection(caller, await signedConnGrant(cloud, userId, "mcp:oauth"))
      .finally(() => (secondSettled = true));

    await new Promise((resolve) => setTimeout(resolve, 100));
    expect(secondSettled).toBe(false);
    expect(upstream.count(TOKEN_URL)).toBe(1);

    release();
    const [g1, g2] = (await Promise.all([first, second])).map(value);
    expect(upstream.count(TOKEN_URL)).toBe(1);
    expect(g1).toMatchObject({ headers: { Authorization: "Bearer at-refreshed-1" }, generation: 3 });
    expect(g2).toMatchObject({ headers: { Authorization: "Bearer at-refreshed-1" }, generation: 3 });
    expect((await storedConnection(userId, "mcp:oauth"))?.generation).toBe(3);

    // Verify form-urlencoded params and redirect: "manual"
    const call0 = upstream.calls[0]!;
    expect(call0.init?.redirect).toBe("manual");
    const params = new URLSearchParams(call0.body);
    expect(params.get("grant_type")).toBe("refresh_token");
    expect(params.get("refresh_token")).toBe("rt-1");
    expect(params.get("client_id")).toBe("cid-1");
    expect(params.get("client_secret")).toBe("csec-1");
    expect(params.get("scope")).toBe("read write");
    expect(params.get("resource")).toBe("https://mcp.example.com/v1");
    vi.restoreAllMocks();

    // 3. Force refresh uses the rotated refresh_token ("rt-2-rotated") and preserves it if omitted from response
    const upstream2 = mockUpstream({
      [TOKEN_URL]: () => json({ access_token: "at-refreshed-2", expires_in: 3600 })
    });
    const g3 = value(await vault().grantConnection(caller, await signedConnGrant(laptop, userId, "mcp:oauth", undefined, true)));
    expect(g3).toMatchObject({ headers: { Authorization: "Bearer at-refreshed-2" }, generation: 4 });
    expect(new URLSearchParams(upstream2.calls[0]!.body).get("refresh_token")).toBe("rt-2-rotated");
    vi.restoreAllMocks();

    // Another force refresh proves rt-2-rotated was preserved when omitted
    const upstream3 = mockUpstream({
      [TOKEN_URL]: () => json({ access_token: "at-refreshed-3", expires_in: 3600 })
    });
    const g4 = value(await vault().grantConnection(caller, await signedConnGrant(laptop, userId, "mcp:oauth", undefined, true)));
    expect(g4).toMatchObject({ headers: { Authorization: "Bearer at-refreshed-3" }, generation: 5 });
    expect(new URLSearchParams(upstream3.calls[0]!.body).get("refresh_token")).toBe("rt-2-rotated");
  });

  it("marks connection reauth_required with lastError on 400 invalid_grant or 401", async () => {
    const { userId, caller, device } = await setup();
    value(
      await vault().putConnection(caller, "mcp:reauth", {
        name: "Reauth Test",
        url: "https://mcp.example.com/v1",
        authKind: "oauth",
        material: {
          kind: "oauth",
          accessToken: "at-old",
          refreshToken: "rt-dead",
          expiresAt: Date.now() + 60_000,
          tokenEndpoint: TOKEN_URL,
          clientId: "cid-1"
        }
      })
    );

    const upstream = mockUpstream({
      [TOKEN_URL]: () => json({ error: "invalid_grant", error_description: "refresh token revoked" }, 400)
    });

    expect(await vault().grantConnection(caller, await signedConnGrant(device, userId, "mcp:reauth"))).toMatchObject({
      ok: false,
      error: "needs_reconnect",
      status: 409
    });
    // Subsequent grant stops hitting upstream once marked reauth_required
    expect(await vault().grantConnection(caller, await signedConnGrant(device, userId, "mcp:reauth"))).toMatchObject({
      ok: false,
      error: "needs_reconnect",
      status: 409
    });
    expect(upstream.count(TOKEN_URL)).toBe(1);

    const listed = value(await vault().listConnections(caller));
    const conn = listed.connections.find((c) => c.id === "mcp:reauth");
    expect(conn?.status).toBe("reauth_required");
    expect(conn?.lastError).toContain("invalid_grant");
  });

  it("refuses SSRF URLs on putConnection, updateConnection, and 3xx redirects during refresh", async () => {
    const { userId, caller, device } = await setup();
    const unsafeUrls = [
      "http://example.com/mcp",
      "https://127.0.0.1/mcp",
      "https://169.254.169.254/latest/meta-data/",
      "https://100.100.100.200/latest/meta-data/",
      "https://[::1]/mcp",
      "https://localhost/mcp",
      "https://metadata.google.internal/computeMetadata/v1/",
      "https://user:pass@example.com/mcp",
      "https://0x7f000001/mcp"
    ];
    for (const url of unsafeUrls) {
      expect(
        await vault().putConnection(caller, "mcp:ssrf", { name: "SSRF", url, authKind: "none" }),
        url
      ).toMatchObject({ ok: false, error: "bad_request", status: 400 });
    }

    // Non-443 port rejected for OAuth tokenEndpoint
    expect(
      await vault().putConnection(caller, "mcp:ssrf-port", {
        name: "Bad Port",
        url: "https://mcp.example.com/v1",
        authKind: "oauth",
        material: {
          kind: "oauth",
          accessToken: "at",
          tokenEndpoint: "https://auth.example.com:8443/token",
          clientId: "cid"
        }
      })
    ).toMatchObject({ ok: false, error: "bad_request", status: 400 });

    // Redirect from tokenEndpoint is refused and never followed
    value(
      await vault().putConnection(caller, "mcp:redir", {
        name: "Redirect",
        url: "https://mcp.example.com/v1",
        authKind: "oauth",
        material: {
          kind: "oauth",
          accessToken: "at-expired",
          refreshToken: "rt-1",
          expiresAt: Date.now() - 10_000,
          tokenEndpoint: TOKEN_URL,
          clientId: "cid"
        }
      })
    );
    const upstream = mockUpstream({
      [TOKEN_URL]: () => new Response(null, { status: 302, headers: { location: "https://169.254.169.254/latest" } })
    });
    expect(await vault().grantConnection(caller, await signedConnGrant(device, userId, "mcp:redir"))).toMatchObject({
      ok: false,
      error: "upstream",
      status: 502
    });
    expect(upstream.calls).toHaveLength(1);
    expect(upstream.calls[0]!.init?.redirect).toBe("manual");
  });
});
