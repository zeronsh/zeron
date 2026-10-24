import { env } from "cloudflare:workers";
import { runInDurableObject } from "cloudflare:test";
import { afterEach, describe, expect, it, vi } from "vitest";
import { accountStub, devicesStub } from "../../src/names";
import { CLAUDE_USAGE_URL } from "../../src/providers/usage";
import { enroll, json, mockUpstream, runnerCaller, setup, signedGrant, value, vault, type UpstreamCall } from "./helpers";

afterEach(() => vi.restoreAllMocks());

const HOUR = 60 * 60_000;

/** A user with `count` Claude logins; the last one signed in is active. */
const withClaudes = async (count: number) => {
  const ctx = await setup();
  const tokens: string[] = [];
  for (let i = 0; i < count; i++) {
    const accessToken = `sk-ant-oat01-access-${i}-${crypto.randomUUID()}`;
    tokens.push(accessToken);
    value(
      await vault().putCredential(ctx.caller, "claude", {
        material: {
          claudeAiOauth: {
            accessToken,
            refreshToken: `sk-ant-ort01-refresh-${crypto.randomUUID()}`,
            expiresAt: Date.now() + 8 * HOUR,
            scopes: ["user:inference", "user:profile"],
            subscriptionType: "max"
          }
        },
        authorizedDevices: [ctx.device.deviceId]
      })
    );
  }
  return { ...ctx, tokens };
};

const switchOn = async (ctx: Awaited<ReturnType<typeof setup>>) =>
  value(await vault().setAutoSwitch(ctx.caller, "claude", 90));

const bearer = (call: UpstreamCall) => new Headers(call.init?.headers).get("authorization")?.replace(/^Bearer /, "") ?? "";

/** The usage endpoint answering each token's 5-hour utilization, or 429. */
const usage = (byToken: Record<string, number | 429>) =>
  mockUpstream({
    // Built per call: a Response made outside the account object can't be read in it.
    [CLAUDE_USAGE_URL]: (call) => {
      const answer = byToken[bearer(call)] ?? 0;
      if (answer === 429) return json({ error: { type: "rate_limit_error" } }, 429);
      return json({
        five_hour: { utilization: answer, resets_at: new Date(Date.now() + HOUR).toISOString() },
        seven_day: { utilization: 10, resets_at: new Date(Date.now() + 100 * HOUR).toISOString() }
      });
    }
  });

const grant = async (ctx: Awaited<ReturnType<typeof setup>>, device = ctx.device) =>
  vault().grant(ctx.caller, await signedGrant(device, ctx.userId, "claude"));

describe("auto-switch", () => {
  it("is off unless turned on: no usage read", async () => {
    const ctx = await withClaudes(2);
    const upstream = mockUpstream({});
    expect(value(await grant(ctx)).accessToken).toBe(ctx.tokens[1]);
    expect(upstream.calls).toHaveLength(0);
  });

  it("with one account, reads nothing", async () => {
    const ctx = await withClaudes(1);
    await switchOn(ctx);
    const upstream = mockUpstream({});
    expect(value(await grant(ctx)).accessToken).toBe(ctx.tokens[0]);
    expect(upstream.calls).toHaveLength(0);
  });

  it("below the threshold, the active account stays and its reading is reused", async () => {
    const ctx = await withClaudes(2);
    await switchOn(ctx);
    const upstream = usage({ [ctx.tokens[0]!]: 10, [ctx.tokens[1]!]: 50 });
    expect(value(await grant(ctx)).accessToken).toBe(ctx.tokens[1]);
    expect(value(await grant(ctx)).accessToken).toBe(ctx.tokens[1]);
    expect(upstream.count(CLAUDE_USAGE_URL)).toBe(1);
    expect(bearer(upstream.calls[0]!)).toBe(ctx.tokens[1]);
  });

  it("at the threshold, the account with the most room becomes active", async () => {
    const ctx = await withClaudes(3);
    await switchOn(ctx);
    const upstream = usage({ [ctx.tokens[0]!]: 40, [ctx.tokens[1]!]: 20, [ctx.tokens[2]!]: 95 });
    const activeSlot = async () => value(await vault().accounts(ctx.caller, "claude")).find((a) => a.active)!.slot;
    const full = await activeSlot();
    expect(value(await grant(ctx)).accessToken).toBe(ctx.tokens[1]);
    expect(upstream.count(CLAUDE_USAGE_URL)).toBe(3);
    // Audited apart from the user's own switches.
    const audit = value(await devicesStub(env, ctx.userId).auditLog(ctx.userId));
    expect(audit.find((entry) => entry.event === "auto_activate")).toMatchObject({
      provider: "claude",
      detail: `${await activeSlot()} at 20%, from ${full} at 95%`
    });
    // The new active one is read no sooner than its reading says: no new call.
    expect(value(await grant(ctx)).accessToken).toBe(ctx.tokens[1]);
    expect(upstream.count(CLAUDE_USAGE_URL)).toBe(3);
  });

  it("stays when no account has 5 points more room", async () => {
    const ctx = await withClaudes(2);
    await switchOn(ctx);
    usage({ [ctx.tokens[0]!]: 92, [ctx.tokens[1]!]: 95 });
    expect(value(await grant(ctx)).accessToken).toBe(ctx.tokens[1]);
  });

  it("skips an account that wouldn't serve the active one's devices and one that needs a reconnect, reading nothing", async () => {
    const ctx = await withClaudes(3);
    await switchOn(ctx);
    const [unauthorized, broken] = value(await vault().accounts(ctx.caller, "claude")).filter((a) => !a.active);
    await runInDurableObject(accountStub(env, ctx.userId, "claude"), (_instance, state) => {
      state.storage.sql.exec("UPDATE accounts SET authorized_devices = '[]' WHERE slot = ?", unauthorized!.slot);
      state.storage.sql.exec("UPDATE accounts SET status = 'needs_reconnect' WHERE slot = ?", broken!.slot);
    });
    const upstream = usage({ [ctx.tokens[0]!]: 0, [ctx.tokens[1]!]: 0, [ctx.tokens[2]!]: 99 });
    expect(value(await grant(ctx)).accessToken).toBe(ctx.tokens[2]);
    expect(upstream.count(CLAUDE_USAGE_URL)).toBe(0);
  });

  it("an unknown usage never switches, and is asked again only after 5 minutes", async () => {
    const ctx = await withClaudes(2);
    await switchOn(ctx);
    const upstream = usage({ [ctx.tokens[0]!]: 0, [ctx.tokens[1]!]: 429 });
    expect(value(await grant(ctx)).accessToken).toBe(ctx.tokens[1]);
    expect(value(await grant(ctx)).accessToken).toBe(ctx.tokens[1]);
    expect(upstream.count(CLAUDE_USAGE_URL)).toBe(1);
    // A 429 never marks the login.
    expect(value(await vault().accounts(ctx.caller, "claude")).map((a) => a.status)).toEqual(["connected", "connected"]);
  });

  it("a 429's Retry-After holds the next read off past it", async () => {
    const ctx = await withClaudes(2);
    await switchOn(ctx);
    mockUpstream({ [CLAUDE_USAGE_URL]: () => new Response("", { status: 429, headers: { "retry-after": "3600" } }) });
    const before = Date.now();
    value(await grant(ctx));
    const active = value(await vault().accounts(ctx.caller, "claude")).find((a) => a.active)!;
    const reading = await runInDurableObject(accountStub(env, ctx.userId, "claude"), (instance) =>
      (instance as unknown as { readings: Map<string, { staleAt: number }> }).readings.get(active.slot)
    );
    // The hour it asked, plus the failed-read backoff.
    expect(reading?.staleAt).toBeGreaterThanOrEqual(before + 65 * 60_000);
  });

  it("the usage view's reading serves the next grant", async () => {
    const ctx = await withClaudes(2);
    await switchOn(ctx);
    const upstream = usage({ [ctx.tokens[0]!]: 10, [ctx.tokens[1]!]: 30 });
    const active = value(await vault().accounts(ctx.caller, "claude")).find((a) => a.active)!;
    value(await vault().accountUsage(ctx.caller, "claude", active.slot));
    expect(value(await grant(ctx)).accessToken).toBe(ctx.tokens[1]);
    expect(upstream.count(CLAUDE_USAGE_URL)).toBe(1);
  });

  it("a slow usage read holds a grant 2 s at most, and its outcome serves the next grant", async () => {
    const ctx = await withClaudes(2);
    await switchOn(ctx);
    let release!: () => void;
    const held = new Promise<void>((resolve) => (release = resolve));
    const upstream = mockUpstream({
      [CLAUDE_USAGE_URL]: async (call) => {
        if (bearer(call) === ctx.tokens[0]) return json({ five_hour: { utilization: 10 } });
        await held;
        return json({ five_hour: { utilization: 95 } });
      }
    });

    // Answers while the active account's read is still held: release() comes after.
    expect(value(await grant(ctx)).accessToken).toBe(ctx.tokens[1]);
    release();
    // The check carried on after the grant: its switch lands on the next one.
    await vi.waitFor(async () => expect(value(await grant(ctx)).accessToken).toBe(ctx.tokens[0]));
    expect(upstream.count(CLAUDE_USAGE_URL)).toBe(2);
  });

  it("an account forgotten during the reads is never made active", async () => {
    const ctx = await withClaudes(2);
    await switchOn(ctx);
    let release!: () => void;
    const held = new Promise<void>((resolve) => (release = resolve));
    mockUpstream({
      [CLAUDE_USAGE_URL]: async (call) => {
        if (bearer(call) === ctx.tokens[1]) return json({ five_hour: { utilization: 95 } });
        await held;
        return json({ five_hour: { utilization: 10 } });
      }
    });

    // The candidate's read hangs; the grant goes on after 2 s.
    expect(value(await grant(ctx)).accessToken).toBe(ctx.tokens[1]);
    const candidate = value(await vault().accounts(ctx.caller, "claude")).find((a) => !a.active)!;
    value(await vault().forgetAccount(ctx.caller, "claude", candidate.slot));
    release();
    expect(value(await grant(ctx)).accessToken).toBe(ctx.tokens[1]);
    expect(value(await vault().accounts(ctx.caller, "claude")).map((a) => a.active)).toEqual([true]);
  });

  it("concurrent grants share one check", async () => {
    const ctx = await withClaudes(2);
    await switchOn(ctx);
    const cloud = await enroll(ctx.caller, "cloud");
    value(await vault().authorize(ctx.caller, "claude", [ctx.device.deviceId, cloud.deviceId]));
    let release!: () => void;
    const held = new Promise<void>((resolve) => (release = resolve));
    const upstream = mockUpstream({
      [CLAUDE_USAGE_URL]: async () => {
        await held;
        return json({ five_hour: { utilization: 50 } });
      }
    });

    const first = grant(ctx);
    await vi.waitFor(() => expect(upstream.count(CLAUDE_USAGE_URL)).toBe(1));
    let secondSettled = false;
    const second = grant(ctx, cloud).finally(() => (secondSettled = true));
    await new Promise((resolve) => setTimeout(resolve, 100));
    expect(secondSettled).toBe(false);
    release();
    for (const result of await Promise.all([first, second])) expect(value(result).accessToken).toBe(ctx.tokens[1]);
    expect(upstream.count(CLAUDE_USAGE_URL)).toBe(1);
  });
});

describe("setAutoSwitch", () => {
  const claudeConnection = (status: { connections: readonly { provider: string }[] }) =>
    status.connections.find((c) => c.provider === "claude");

  it("turns it on and off per provider, shows it on the connection, and audits both", async () => {
    const ctx = await withClaudes(1);
    expect(claudeConnection(value(await vault().setAutoSwitch(ctx.caller, "claude", 90)))).toMatchObject({ autoSwitch: 90 });
    expect(claudeConnection(value(await vault().status(ctx.caller)))).toMatchObject({ autoSwitch: 90 });
    expect(claudeConnection(value(await vault().setAutoSwitch(ctx.caller, "claude", null)))).not.toHaveProperty("autoSwitch");
    const audit = value(await devicesStub(env, ctx.userId).auditLog(ctx.userId));
    expect(audit.filter((entry) => entry.event === "auto_switch").map((entry) => entry.detail).sort()).toEqual(["90%", "off"]);
  });

  it("refuses a bad threshold, a provider without plan usage, a runner, and no credential", async () => {
    const ctx = await withClaudes(1);
    for (const threshold of [0, 101, 50.5, Number.NaN, "90" as unknown as number]) {
      expect(await vault().setAutoSwitch(ctx.caller, "claude", threshold)).toMatchObject({ ok: false, error: "bad_request" });
    }
    expect(await vault().setAutoSwitch(ctx.caller, "anthropic-key", 90)).toMatchObject({ ok: false, error: "bad_request" });
    expect(await vault().setAutoSwitch(runnerCaller(ctx.userId, ctx.device.deviceId), "claude", 90)).toMatchObject({
      ok: false,
      error: "forbidden"
    });
    expect(await vault().setAutoSwitch(ctx.caller, "codex", 90)).toMatchObject({ ok: false, error: "not_found" });
  });
});
