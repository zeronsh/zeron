import { exports } from "cloudflare:workers";
import { afterEach, describe, expect, it, vi } from "vitest";
import type { GrantManyRequest, VaultProviderId } from "../../src/api";
import { ANTIGRAVITY_TIER_URL, ANTIGRAVITY_USAGE_URL, ANTIGRAVITY_USER_AGENT } from "../../src/providers/usage";
import { json, mockUpstream, nextTs, runnerCaller, setup, signedGrant, value, vault } from "./helpers";
import { fakeJwt, type TestDevice } from "../support";

afterEach(() => vi.restoreAllMocks());

const HOUR = 60 * 60_000;
const TOKEN_URL = "https://auth.x.ai/oauth2/token";

const batch = async (
  device: TestDevice,
  userId: string,
  providers: readonly VaultProviderId[],
  signed: readonly string[] = providers,
  ts = nextTs()
): Promise<GrantManyRequest> => ({
  providers,
  deviceId: device.deviceId,
  ts,
  sig: await device.sign(`zeron-vault-grant\n${userId}\n${device.deviceId}\n${signed.join(",")}\n${ts}`)
});

const grokMaterial = (accessToken: string, refreshToken: string, expiresAt: number) => ({
  grokAuth: {
    key: accessToken,
    refresh_token: refreshToken,
    expires_at: new Date(expiresAt).toISOString(),
    oidc_issuer: "https://auth.x.ai",
    oidc_client_id: "client-1",
    user_id: "user-1",
    email: "ada@x.ai"
  },
  scope: "https://auth.x.ai::client-1"
});

type Ctx = Awaited<ReturnType<typeof setup>>;
const put = async (ctx: Ctx, provider: VaultProviderId, material: Record<string, unknown>) =>
  value(await vault().putCredential(ctx.caller, provider, { material: material as never, authorizedDevices: [ctx.device.deviceId] }));

describe("grantMany", () => {
  it("grants every connected provider in one signed call and lists the rest as missing", async () => {
    const ctx = await setup();
    await put(ctx, "openrouter-key", { key: "sk-or-v1-batch-0001" });
    await put(ctx, "copilot", { copilotToken: "gho_abcdefghijklmnopqrstuvwxyz0123", login: "octocat" });
    await put(ctx, "grok", grokMaterial("xai-access-1", "xai-refresh-1", Date.now() + 2 * HOUR));

    const reply = value(
      await vault().grantMany(ctx.caller, await batch(ctx.device, ctx.userId, ["copilot", "deepseek-key", "grok", "openrouter-key"]))
    );
    expect(reply.grants.map((g) => [g.provider, g.accessToken])).toEqual([
      ["copilot", "gho_abcdefghijklmnopqrstuvwxyz0123"],
      ["grok", "xai-access-1"],
      ["openrouter-key", "sk-or-v1-batch-0001"]
    ]);
    expect(reply.grants.find((g) => g.provider === "grok")?.settings).toMatchObject({
      scope: "https://auth.x.ai::client-1",
      user_id: "user-1"
    });
    expect(reply.missing).toEqual([{ provider: "deepseek-key", error: "not_found", message: "no deepseek-key credential" }]);
  });

  it("works through the VaultApi entrypoint (real RPC)", async () => {
    const ctx = await setup();
    await put(ctx, "xai-key", { key: "xai-key-rpc-0001" });
    const reply = await exports.VaultApi.grantMany(ctx.caller, await batch(ctx.device, ctx.userId, ["xai-key"]));
    expect(reply.ok && reply.value.grants[0]?.accessToken).toBe("xai-key-rpc-0001");
  });

  it("refuses a list that isn't sorted and distinct, or names github", async () => {
    const ctx = await setup();
    for (const providers of [["xai-key", "copilot"], ["copilot", "copilot"], ["copilot", "github"], []] as VaultProviderId[][]) {
      const result = await vault().grantMany(ctx.caller, await batch(ctx.device, ctx.userId, providers));
      expect(result.ok ? "ok" : result.error, providers.join(",")).toBe("bad_request");
    }
  });

  it("the signature covers the whole list, and one ts is spent once", async () => {
    const ctx = await setup();
    await put(ctx, "xai-key", { key: "xai-key-sig-0001" });
    const forged = await batch(ctx.device, ctx.userId, ["copilot", "xai-key"], ["xai-key"]);
    const refused = await vault().grantMany(ctx.caller, forged);
    expect(refused.ok ? "ok" : refused.error).toBe("bad_signature");

    const ts = nextTs();
    expect((await vault().grantMany(ctx.caller, await batch(ctx.device, ctx.userId, ["xai-key"], ["xai-key"], ts))).ok).toBe(true);
    const replay = await vault().grantMany(ctx.caller, await batch(ctx.device, ctx.userId, ["xai-key"], ["xai-key"], ts));
    expect(replay.ok ? "ok" : replay.error).toBe("stale");
    // The fence is the device's, shared with single grants.
    const single = await vault().grant(ctx.caller, await signedGrant(ctx.device, ctx.userId, "xai-key", ts));
    expect(single.ok ? "ok" : single.error).toBe("stale");
  });

  it("a runner draws only its own device's grants; an unauthorized device is refused per provider", async () => {
    const ctx = await setup();
    await put(ctx, "gemini-key", { key: "gemini-key-0000001" });
    const other = runnerCaller(ctx.userId, "some-other-device");
    const forbidden = await vault().grantMany(other, await batch(ctx.device, ctx.userId, ["gemini-key"]));
    expect(forbidden.ok ? "ok" : forbidden.error).toBe("forbidden");

    value(await vault().authorize(ctx.caller, "gemini-key", ["somewhere-else"]));
    const reply = value(await vault().grantMany(ctx.caller, await batch(ctx.device, ctx.userId, ["gemini-key"])));
    expect(reply.missing.map((m) => m.error)).toEqual(["not_authorized"]);
  });
});

describe("grok in the vault", () => {
  it("refreshes at xAI once due, persisting the rotated refresh token first", async () => {
    const ctx = await setup();
    // Ten minutes left: inside the 30-minute margin an upload is held to.
    await put(ctx, "grok", grokMaterial("xai-access-1", "xai-refresh-1", Date.now() + 10 * 60_000));
    let refreshes = 0;
    const upstream = mockUpstream({
      [TOKEN_URL]: ({ body }) => {
        refreshes++;
        const form = new URLSearchParams(body);
        expect(form.get("client_id")).toBe("client-1");
        return form.get("refresh_token") === `xai-refresh-${refreshes}`
          ? json({ access_token: `xai-access-${refreshes + 1}`, refresh_token: `xai-refresh-${refreshes + 1}`, expires_in: 3600 })
          : json({ error: "invalid_grant" }, 400);
      }
    });
    const grant = value(await vault().grant(ctx.caller, await signedGrant(ctx.device, ctx.userId, "grok")));
    expect(grant.accessToken).toBe("xai-access-2");
    expect(grant.expiresAt).toBeGreaterThan(Date.now() + 50 * 60_000);
    // Fresh now: no second refresh.
    value(await vault().grant(ctx.caller, await signedGrant(ctx.device, ctx.userId, "grok")));
    expect(upstream.count(TOKEN_URL)).toBe(1);
  });

  it("a refused refresh token marks the account for reconnecting", async () => {
    const ctx = await setup();
    await put(ctx, "grok", grokMaterial("xai-access-1", "xai-refresh-dead", Date.now() - 60_000));
    mockUpstream({ [TOKEN_URL]: () => json({ error: "invalid_grant" }, 400) });
    const refused = await vault().grant(ctx.caller, await signedGrant(ctx.device, ctx.userId, "grok"));
    expect(refused.ok ? "ok" : refused.error).toBe("needs_reconnect");
    const accounts = value(await vault().accounts(ctx.caller, "grok"));
    expect(accounts[0]).toMatchObject({ status: "needsReconnect", kind: "oauth", account: "ada@x.ai" });
  });

  it("the same Grok user signed in again replaces its account", async () => {
    const ctx = await setup();
    await put(ctx, "grok", grokMaterial("xai-access-1", "xai-refresh-1", Date.now() + 2 * HOUR));
    await put(ctx, "grok", grokMaterial("xai-access-9", "xai-refresh-9", Date.now() + 2 * HOUR));
    expect(value(await vault().accounts(ctx.caller, "grok"))).toHaveLength(1);
  });
});

describe("other sign-ins", () => {
  it("a Cursor sign-in key turns needs-reconnect at its expiry", async () => {
    const ctx = await setup();
    const expiresAt = Date.now() + 30 * 60_000;
    await put(ctx, "cursor-key", { key: "crsr_abcdefgh12345678", expiresAt });
    const grant = value(await vault().grant(ctx.caller, await signedGrant(ctx.device, ctx.userId, "cursor-key")));
    expect(grant.expiresAt).toBe(expiresAt);
    vi.spyOn(Date, "now").mockReturnValue(expiresAt + 1);
    const status = value(await vault().status(ctx.caller));
    expect(status.connections.find((c) => c.provider === "cursor-key")?.status).toBe("needsReconnect");
  });

  it("Devin keeps its servers beside the key and lists as a sign-in", async () => {
    const ctx = await setup();
    await put(ctx, "devin", {
      devinCredentials: { windsurf_api_key: "devin-key-0123456789", api_server_url: "https://server.codeium.com" }
    });
    const grant = value(await vault().grant(ctx.caller, await signedGrant(ctx.device, ctx.userId, "devin")));
    expect(grant).toMatchObject({ accessToken: "devin-key-0123456789", settings: { api_server_url: "https://server.codeium.com" } });
    expect(value(await vault().accounts(ctx.caller, "devin"))[0]?.kind).toBe("oauth");
    expect(value(await vault().accounts(ctx.caller, "xai-key"))).toEqual([]);
  });

  it("vendor keys list as API keys", async () => {
    const ctx = await setup();
    await put(ctx, "ai-gateway-key", { key: "vck_gateway_000001" });
    expect(value(await vault().accounts(ctx.caller, "ai-gateway-key"))[0]).toMatchObject({ kind: "api-key", account: "…0001" });
  });
});

describe("Hermes, Antigravity and Muse sign-ins", () => {
  it("grants every login in one batch, refreshing Antigravity's at Google first", async () => {
    const ctx = await setup();
    const exp = (ms: number) => Math.floor(ms / 1000);
    await put(ctx, "nous", {
      nousAuth: {
        access_token: fakeJwt({ exp: exp(Date.now() + 5 * HOUR), email: "ada@nous.example" }),
        refresh_token: "nous-rt",
        portal_base_url: "https://portal.nousresearch.com",
        inference_base_url: "https://inference-api.nousresearch.com/v1"
      }
    });
    await put(ctx, "xai-oauth", {
      xaiOauth: { tokens: { access_token: fakeJwt({ exp: exp(Date.now() + 6 * HOUR), sub: "u1" }), refresh_token: "xai-rt" } }
    });
    await put(ctx, "antigravity", {
      antigravityToken: {
        client_id: "1071006060591-abc.apps.googleusercontent.com",
        client_secret: "GOCSPX-test",
        refresh_token: "1//google-rt",
        scopes: ["https://www.googleapis.com/auth/cloud-platform"],
        project_id: "proj-1"
      }
    });
    await put(ctx, "muse", {
      museAuth: { providers: { meta: { access_token: "meta-at", refresh_token: "meta-rt", expires_at: exp(Date.now() + 30 * 24 * HOUR) } } }
    });
    const upstream = mockUpstream({
      "https://oauth2.googleapis.com/token": () => json({ access_token: "ya29.fresh", expires_in: 3599 })
    });
    const reply = value(
      await vault().grantMany(ctx.caller, await batch(ctx.device, ctx.userId, ["antigravity", "muse", "nous", "xai-oauth"]))
    );
    expect(reply.missing).toEqual([]);
    const by = Object.fromEntries(reply.grants.map((g) => [g.provider, g]));
    expect(by.antigravity?.accessToken).toBe("ya29.fresh");
    expect(by.nous?.settings?.portal_base_url).toBe("https://portal.nousresearch.com");
    expect(JSON.parse(by.muse!.settings!.document!).providers.meta.refresh_token).toBe("");
    expect(upstream.count("https://oauth2.googleapis.com/token")).toBe(1);
    // Nothing a machine gets holds a refresh token.
    expect(JSON.stringify(reply)).not.toMatch(/nous-rt|xai-rt|google-rt|meta-rt/);
  });

  it("reads an Antigravity login's limits for its project, with its tiers beside them", async () => {
    const ctx = await setup();
    await put(ctx, "antigravity", {
      antigravityToken: {
        client_id: "1071006060591-abc.apps.googleusercontent.com",
        client_secret: "GOCSPX-test",
        refresh_token: "1//google-rt",
        scopes: ["https://www.googleapis.com/auth/cloud-platform"],
        project_id: "proj-1"
      }
    });
    const slot = value(await vault().accounts(ctx.caller, "antigravity"))[0]!.slot;
    const groups = [{ displayName: "Claude and GPT models", buckets: [{ window: "5h", remainingFraction: 0.4 }] }];
    let tier = () => json({ cloudaicompanionProject: "proj-assist", paidTier: { id: "g1-ultra-tier", name: "Google AI Ultra" } });
    const upstream = mockUpstream({
      "https://oauth2.googleapis.com/token": () => json({ access_token: "ya29.fresh", expires_in: 3599 }),
      [ANTIGRAVITY_TIER_URL]: () => tier(),
      [ANTIGRAVITY_USAGE_URL]: () => json({ groups })
    });
    const usage = value(await vault().accountUsage(ctx.caller, "antigravity", slot));
    expect(usage.body).toEqual({ groups, paidTier: { id: "g1-ultra-tier", name: "Google AI Ultra" } });
    const asked = upstream.calls.find((c) => c.url === ANTIGRAVITY_USAGE_URL)!;
    expect(new Headers(asked.init?.headers).get("authorization")).toBe("Bearer ya29.fresh");
    expect(new Headers(asked.init?.headers).get("user-agent")).toBe(ANTIGRAVITY_USER_AGENT);
    // The login's own project, not the one loadCodeAssist would pick.
    expect(JSON.parse(String(asked.init?.body))).toEqual({ project: "proj-1" });

    // Refused by Google: an upstream failure, the login left connected.
    tier = () => json({ error: { code: 403 } }, 403);
    const refused = await vault().accountUsage(ctx.caller, "antigravity", slot);
    expect(!refused.ok && refused.error).toBe("upstream");
    expect(value(await vault().accounts(ctx.caller, "antigravity"))[0]!.status).toBe("connected");
  });
});
