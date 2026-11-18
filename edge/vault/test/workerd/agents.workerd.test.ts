import { exports } from "cloudflare:workers";
import { afterEach, describe, expect, it, vi } from "vitest";
import type { GrantManyRequest, VaultProviderId } from "../../src/api";
import { json, mockUpstream, nextTs, runnerCaller, setup, signedGrant, value, vault } from "./helpers";
import type { TestDevice } from "../support";

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
