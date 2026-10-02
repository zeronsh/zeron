import { env } from "cloudflare:workers";
import { runInDurableObject } from "cloudflare:test";
import { afterEach, describe, expect, it, vi } from "vitest";
import { jwtExpMs } from "../../src/jwt";
import { accountStub } from "../../src/names";
import { CODEX_CLIENT_ID, CODEX_TOKEN_URL } from "../../src/providers/codex";
import { fakeJwt } from "../support";
import {
  DAY,
  codexMaterial,
  codexTokens,
  enroll,
  json,
  mockUpstream,
  sec,
  setup,
  signedGrant,
  value,
  vault
} from "./helpers";

afterEach(() => vi.restoreAllMocks());

/** A user with a codex credential whose access token expires in `expiresInMs`. */
const withCodex = async (expiresInMs: number) => {
  const ctx = await setup();
  const tokens = codexTokens(Date.now() + expiresInMs);
  value(
    await vault().putCredential(ctx.caller, "codex", {
      material: codexMaterial(tokens),
      authorizedDevices: [ctx.device.deviceId]
    })
  );
  return { ...ctx, tokens };
};

const storedGeneration = (userId: string) =>
  runInDurableObject(accountStub(env, userId, "codex"), (_instance, state) =>
    state.storage.sql.exec<{ generation: number }>("SELECT generation FROM record").toArray()[0]?.generation
  );

describe("codex refresh", () => {
  it("does not refresh a token with more than an hour left", async () => {
    const { userId, caller, device, tokens } = await withCodex(5 * DAY);
    const upstream = mockUpstream({});
    const grant = value(await vault().grant(caller, await signedGrant(device, userId, "codex")));
    expect(grant).toMatchObject({
      accessToken: tokens.access_token,
      idToken: tokens.id_token,
      accountId: "acct-ada",
      account: "ada@example.com",
      generation: 1,
      // The token's own exp claim, not a re-computed clock (second boundaries).
      expiresAt: jwtExpMs(tokens.access_token)
    });
    expect(upstream.calls).toHaveLength(0);
  });

  it("two concurrent grants share exactly ONE upstream refresh", async () => {
    const { userId, caller, device, tokens } = await withCodex(30 * 60_000);
    const cloud = await enroll(caller, "cloud");
    value(await vault().authorize(caller, "codex", [device.deviceId, cloud.deviceId]));

    const fresh = fakeJwt({ exp: sec(Date.now() + 10 * DAY), jti: "fresh" });
    let release!: () => void;
    const held = new Promise<void>((resolve) => (release = resolve));
    const upstream = mockUpstream({
      [CODEX_TOKEN_URL]: async () => {
        await held;
        return json({ access_token: fresh, refresh_token: "rt-rotated" });
      }
    });

    const first = vault().grant(caller, await signedGrant(device, userId, "codex"));
    await vi.waitFor(() => expect(upstream.count(CODEX_TOKEN_URL)).toBe(1));
    let secondSettled = false;
    const second = vault()
      .grant(caller, await signedGrant(cloud, userId, "codex"))
      .finally(() => (secondSettled = true));
    // The second grant reaches the account object and parks on the in-flight
    // refresh instead of starting its own.
    await new Promise((resolve) => setTimeout(resolve, 100));
    expect(secondSettled).toBe(false);
    expect(upstream.count(CODEX_TOKEN_URL)).toBe(1);

    release();
    const [a, b] = (await Promise.all([first, second])).map(value);
    expect(upstream.count(CODEX_TOKEN_URL)).toBe(1);
    expect(a).toMatchObject({ accessToken: fresh, generation: 2 });
    expect(b).toMatchObject({ accessToken: fresh, generation: 2 });
    expect(await storedGeneration(userId)).toBe(2);

    const body = JSON.parse(upstream.calls[0]!.body);
    expect(body).toEqual({
      client_id: CODEX_CLIENT_ID,
      grant_type: "refresh_token",
      refresh_token: tokens.refresh_token,
      scope: "openid profile email"
    });

    // The rotated chain is what the next refresh would use; no refresh now.
    const third = value(await vault().grant(caller, await signedGrant(device, userId, "codex")));
    expect(third).toMatchObject({ accessToken: fresh, generation: 2, idToken: tokens.id_token });
    expect(upstream.count(CODEX_TOKEN_URL)).toBe(1);
  });

  it("invalid_grant marks the account needs_reconnect and stops issuing", async () => {
    const { userId, caller, device } = await withCodex(30 * 60_000);
    const upstream = mockUpstream({
      [CODEX_TOKEN_URL]: () => json({ error: "invalid_grant", error_description: "reused" }, 400)
    });
    expect(await vault().grant(caller, await signedGrant(device, userId, "codex"))).toMatchObject({
      ok: false,
      error: "needs_reconnect",
      status: 409
    });
    expect(await vault().grant(caller, await signedGrant(device, userId, "codex"))).toMatchObject({
      ok: false,
      error: "needs_reconnect"
    });
    expect(upstream.count(CODEX_TOKEN_URL)).toBe(1);
    const status = value(await vault().status(caller));
    expect(status.connections.find((c) => c.provider === "codex")?.status).toBe("needsReconnect");
    expect(await storedGeneration(userId)).toBe(1);

    // Reconnecting (a fresh upload) clears it.
    value(
      await vault().putCredential(caller, "codex", {
        material: codexMaterial(codexTokens(Date.now() + 5 * DAY)),
        authorizedDevices: [device.deviceId]
      })
    );
    expect(value(await vault().grant(caller, await signedGrant(device, userId, "codex"))).generation).toBe(2);
  });

  it("a 401 from the token endpoint is also a reconnect", async () => {
    const { userId, caller, device } = await withCodex(30 * 60_000);
    mockUpstream({ [CODEX_TOKEN_URL]: () => json({ error: { code: "refresh_token_expired" } }, 401) });
    expect(await vault().grant(caller, await signedGrant(device, userId, "codex"))).toMatchObject({
      ok: false,
      error: "needs_reconnect"
    });
  });

  it("a 503 keeps the generation and returns the still-valid token", async () => {
    const { userId, caller, device, tokens } = await withCodex(30 * 60_000);
    const upstream = mockUpstream({ [CODEX_TOKEN_URL]: () => new Response("upstream down", { status: 503 }) });
    const grant = value(await vault().grant(caller, await signedGrant(device, userId, "codex")));
    expect(grant).toMatchObject({ accessToken: tokens.access_token, generation: 1 });
    expect(await storedGeneration(userId)).toBe(1);
    // Backoff: an immediate re-grant serves the same token without hammering upstream.
    value(await vault().grant(caller, await signedGrant(device, userId, "codex")));
    expect(upstream.count(CODEX_TOKEN_URL)).toBe(1);
    const status = value(await vault().status(caller));
    expect(status.connections.find((c) => c.provider === "codex")?.status).toBe("connected");
  });

  it("a 503 with an already-expired token is an upstream error", async () => {
    const { userId, caller, device } = await withCodex(-60_000);
    mockUpstream({ [CODEX_TOKEN_URL]: () => new Response("upstream down", { status: 503 }) });
    expect(await vault().grant(caller, await signedGrant(device, userId, "codex"))).toMatchObject({
      ok: false,
      error: "upstream",
      status: 502
    });
  });

  it("a network failure is transient too", async () => {
    const { userId, caller, device, tokens } = await withCodex(30 * 60_000);
    mockUpstream({
      [CODEX_TOKEN_URL]: () => {
        throw new TypeError("connection reset");
      }
    });
    expect(value(await vault().grant(caller, await signedGrant(device, userId, "codex"))).accessToken).toBe(
      tokens.access_token
    );
  });

  it("an upload landing mid-refresh wins over the refreshed tokens", async () => {
    const { userId, caller, device } = await withCodex(30 * 60_000);
    let release!: () => void;
    const held = new Promise<void>((resolve) => (release = resolve));
    const upstream = mockUpstream({
      [CODEX_TOKEN_URL]: async () => {
        await held;
        return json({ access_token: fakeJwt({ exp: sec(Date.now() + 10 * DAY) }), refresh_token: "rt-from-refresh" });
      }
    });
    const pending = vault().grant(caller, await signedGrant(device, userId, "codex"));
    await vi.waitFor(() => expect(upstream.count(CODEX_TOKEN_URL)).toBe(1));
    const uploaded = codexTokens(Date.now() + 5 * DAY);
    value(
      await vault().putCredential(caller, "codex", {
        material: codexMaterial(uploaded),
        authorizedDevices: [device.deviceId]
      })
    );
    release();
    const grant = value(await pending);
    expect(grant).toMatchObject({ accessToken: uploaded.access_token, generation: 2 });
    expect(await storedGeneration(userId)).toBe(2);
  });
});
