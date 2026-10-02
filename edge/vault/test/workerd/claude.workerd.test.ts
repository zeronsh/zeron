import { env } from "cloudflare:workers";
import { runInDurableObject } from "cloudflare:test";
import { afterEach, describe, expect, it, vi } from "vitest";
import { runCanary } from "../../src/canary";
import { accountStub } from "../../src/names";
import { CLAUDE_CLIENT_ID, CLAUDE_TOKEN_URL } from "../../src/providers/claude";
import { DAY, enroll, json, mockUpstream, setup, signedGrant, value, vault } from "./helpers";

afterEach(() => vi.restoreAllMocks());

const MINUTE = 60_000;

const oauthBlob = (expiresInMs: number) => ({
  accessToken: `sk-ant-oat01-access-${crypto.randomUUID()}`,
  refreshToken: `sk-ant-ort01-refresh-${crypto.randomUUID()}`,
  expiresAt: Date.now() + expiresInMs,
  scopes: ["user:inference", "user:profile"],
  subscriptionType: "max"
});

/** A user whose Claude OAuth login expires in `expiresInMs`. */
const withClaude = async (expiresInMs: number) => {
  const ctx = await setup();
  const blob = oauthBlob(expiresInMs);
  value(
    await vault().putCredential(ctx.caller, "claude", {
      material: { claudeAiOauth: blob },
      authorizedDevices: [ctx.device.deviceId]
    })
  );
  return { ...ctx, blob };
};

const stored = (userId: string) =>
  runInDurableObject(accountStub(env, userId, "claude"), (_instance, state) =>
    state.storage.sql
      .exec<{ generation: number; status: string; envelope: string }>("SELECT generation, status, envelope FROM record")
      .toArray()[0]
  );

describe("claude OAuth login", () => {
  it("grants the stored access token while more than 10 minutes remain", async () => {
    const { userId, caller, device, blob } = await withClaude(60 * MINUTE);
    const upstream = mockUpstream({});
    expect(value(await vault().grant(caller, await signedGrant(device, userId, "claude")))).toEqual({
      provider: "claude",
      accessToken: blob.accessToken,
      expiresAt: blob.expiresAt,
      scopes: blob.scopes,
      subscriptionType: "max",
      generation: 1
    });
    expect(upstream.calls).toHaveLength(0);
    const status = value(await vault().status(caller));
    expect(status.connections.find((c) => c.provider === "claude")).toMatchObject({ status: "connected", account: "max" });
  });

  it("single-flights the refresh and persists the rotated refresh token", async () => {
    const { userId, caller, device, blob } = await withClaude(5 * MINUTE);
    const cloud = await enroll(caller, "cloud");
    value(await vault().authorize(caller, "claude", [device.deviceId, cloud.deviceId]));

    let release!: () => void;
    const held = new Promise<void>((resolve) => (release = resolve));
    const refreshTokensSeen: string[] = [];
    const answers = [
      // Rotates the refresh token; short lifetime so the canary below can refresh again.
      () => json({ access_token: "sk-ant-oat01-second", refresh_token: "sk-ant-ort01-rotated", expires_in: 3600, scope: "user:inference" }),
      // No refresh token in the answer: the rotated one must be kept.
      () => json({ access_token: "sk-ant-oat01-third", expires_in: 3600 })
    ];
    const upstream = mockUpstream({
      [CLAUDE_TOKEN_URL]: async (call) => {
        refreshTokensSeen.push(JSON.parse(call.body).refresh_token);
        if (refreshTokensSeen.length === 1) await held;
        return answers.shift()!();
      }
    });

    const first = vault().grant(caller, await signedGrant(device, userId, "claude"));
    await vi.waitFor(() => expect(upstream.count(CLAUDE_TOKEN_URL)).toBe(1));
    let secondSettled = false;
    const second = vault()
      .grant(caller, await signedGrant(cloud, userId, "claude"))
      .finally(() => (secondSettled = true));
    await new Promise((resolve) => setTimeout(resolve, 100));
    expect(secondSettled).toBe(false);
    release();
    const [a, b] = (await Promise.all([first, second])).map(value);
    expect(upstream.count(CLAUDE_TOKEN_URL)).toBe(1);
    for (const grant of [a, b]) {
      expect(grant).toMatchObject({ accessToken: "sk-ant-oat01-second", generation: 2, scopes: ["user:inference"], subscriptionType: "max" });
      expect(grant.expiresAt).toBeGreaterThan(Date.now() + 59 * MINUTE);
    }
    expect(JSON.parse(upstream.calls[0]!.body)).toEqual({
      grant_type: "refresh_token",
      refresh_token: blob.refreshToken,
      client_id: CLAUDE_CLIENT_ID
    });
    expect((await stored(userId))?.generation).toBe(2);

    // The next refresh (forced by the canary) uses the ROTATED token, and an
    // answer without a refresh token keeps it.
    const [, claudeResult] = await runCanary({ ...env, CANARY_USER_ID: userId });
    expect(claudeResult).toMatchObject({ provider: "claude", ok: true, status: "refreshed" });
    expect(refreshTokensSeen).toEqual([blob.refreshToken, "sk-ant-ort01-rotated"]);
    expect(value(await vault().grant(caller, await signedGrant(device, userId, "claude")))).toMatchObject({
      accessToken: "sk-ant-oat01-third",
      generation: 3
    });
  });

  it("invalid_grant marks the account needs_reconnect", async () => {
    const { userId, caller, device } = await withClaude(5 * MINUTE);
    const upstream = mockUpstream({
      [CLAUDE_TOKEN_URL]: () => json({ error: "invalid_grant", error_description: "Refresh token not found or invalid" }, 400)
    });
    expect(await vault().grant(caller, await signedGrant(device, userId, "claude"))).toMatchObject({
      ok: false,
      error: "needs_reconnect",
      status: 409
    });
    expect(await vault().grant(caller, await signedGrant(device, userId, "claude"))).toMatchObject({
      ok: false,
      error: "needs_reconnect"
    });
    expect(upstream.count(CLAUDE_TOKEN_URL)).toBe(1);
    const status = value(await vault().status(caller));
    expect(status.connections.find((c) => c.provider === "claude")?.status).toBe("needsReconnect");
  });

  it("a 5xx keeps the still-valid token", async () => {
    const { userId, caller, device, blob } = await withClaude(5 * MINUTE);
    mockUpstream({ [CLAUDE_TOKEN_URL]: () => new Response("overloaded", { status: 529 }) });
    expect(value(await vault().grant(caller, await signedGrant(device, userId, "claude")))).toMatchObject({
      accessToken: blob.accessToken,
      generation: 1
    });
  });
});

describe("claude setup token", () => {
  const TOKEN = "sk-ant-oat01-setup-token-SECRETSECRET-0123456789";

  it("defaults to a one-year expiry and never refreshes", async () => {
    const { userId, caller, device } = await setup();
    const upstream = mockUpstream({});
    const before = Date.now();
    value(await vault().putCredential(caller, "claude", { material: { oauthToken: TOKEN }, authorizedDevices: [device.deviceId] }));
    const grant = value(await vault().grant(caller, await signedGrant(device, userId, "claude")));
    expect(grant).toMatchObject({ provider: "claude", accessToken: TOKEN, generation: 1 });
    expect(grant.expiresAt).toBeGreaterThanOrEqual(before + 365 * DAY);
    expect(grant.scopes).toBeUndefined();
    expect(upstream.calls).toHaveLength(0);

    const status = value(await vault().status(caller));
    expect(status.connections.find((c) => c.provider === "claude")).toMatchObject({
      status: "connected",
      account: "Claude subscription"
    });
    expect(JSON.stringify(status)).not.toContain(TOKEN);
    expect((await stored(userId))?.envelope).not.toContain(TOKEN);

    const [, claudeResult] = await runCanary({ ...env, CANARY_USER_ID: userId });
    expect(claudeResult).toMatchObject({ ok: true, status: "valid" });
  });

  it("past its expiry, the account needs a reconnect", async () => {
    const { userId, caller, device } = await setup();
    value(
      await vault().putCredential(caller, "claude", {
        material: { oauthToken: TOKEN, expiresAt: Date.now() + 50 },
        authorizedDevices: [device.deviceId]
      })
    );
    await new Promise((resolve) => setTimeout(resolve, 80));
    // Visible in status without any grant having to notice first.
    let status = value(await vault().status(caller));
    expect(status.connections.find((c) => c.provider === "claude")?.status).toBe("needsReconnect");
    expect(await vault().grant(caller, await signedGrant(device, userId, "claude"))).toMatchObject({
      ok: false,
      error: "needs_reconnect"
    });
    expect((await stored(userId))?.status).toBe("needs_reconnect");
    status = value(await vault().status(caller));
    expect(status.connections.find((c) => c.provider === "claude")?.status).toBe("needsReconnect");
  });

  it("the canary flags a token with under 30 days left", async () => {
    const { userId, caller, device } = await setup();
    value(
      await vault().putCredential(caller, "claude", {
        material: { oauthToken: TOKEN, expiresAt: Date.now() + 10 * DAY },
        authorizedDevices: [device.deviceId]
      })
    );
    const [, claudeResult] = await runCanary({ ...env, CANARY_USER_ID: userId });
    expect(claudeResult).toMatchObject({ provider: "claude", ok: false, status: "expiring" });
  });

  it("rejects malformed and already-expired uploads", async () => {
    const { caller, device } = await setup();
    const put = (material: Parameters<ReturnType<typeof vault>["putCredential"]>[2]["material"]) =>
      vault().putCredential(caller, "claude", { material, authorizedDevices: [device.deviceId] });
    expect(await put({ oauthToken: "sk-ant-api03-not-a-setup-token" })).toMatchObject({ ok: false, error: "bad_request" });
    expect(await put({ oauthToken: TOKEN, expiresAt: Date.now() - 1000 })).toMatchObject({ ok: false, error: "bad_request" });
    expect(await put({ claudeAiOauth: { accessToken: "a" } })).toMatchObject({ ok: false, error: "bad_request" });
    expect(await put({ key: "sk-ant-api03-wrong-shape" })).toMatchObject({ ok: false, error: "bad_request" });
  });
});
