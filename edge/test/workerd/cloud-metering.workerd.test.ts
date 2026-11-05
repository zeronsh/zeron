import { env, runInDurableObject } from "cloudflare:test";
import { describe, expect, it } from "vitest";
import { exportMonth, handleAdminRoute, scanOrphans } from "../../src/cloud/billing";
import { CLOUD_INDEX_NAME } from "../../src/cloud/cloud-index";
import {
  addCredits,
  dayEnd,
  dayKey,
  dayStart,
  ensureMeteringTables,
  listUsage,
  monthKey,
  recordSandbox
} from "../../src/cloud/metering";
import { cloudAccountName } from "../../src/cloud/policy";
import type { Env } from "../../src/env";
import { boatControl, call, newUser, userBearer, type TestUser } from "./cloud-helpers";

const ALPHABET = "23456789abcdefghjkmnpqrstuvwxyz";
const boatId = () =>
  `bx_${Array.from(crypto.getRandomValues(new Uint8Array(8)), (b) => ALPHABET[b % ALPHABET.length]).join("")}`;
const DAY = 86_400_000;

const deviceStub = (u: TestUser) => env.CLOUD_ACCOUNTS.get(env.CLOUD_ACCOUNTS.idFromName(cloudAccountName(u.orgId, u.userId)));
const index = () => env.CLOUD_INDEX.get(env.CLOUD_INDEX.idFromName(CLOUD_INDEX_NAME));

type DeviceInternals = {
  acct: { orgId?: string; userId?: string; deviceId?: string };
  reconcile(now?: number, full?: boolean): Promise<boolean>;
};

/** A start time earlier today (tests run at any time of day, including
 * right after midnight). */
const earlierToday = (ms: number) => Math.max(dayStart(dayKey(Date.now())), Date.now() - ms);

/** A user whose Cloud history holds one Boat sandbox created at `createdAt`
 * (the fake Boat API knows it too), registered with the billing index. */
const meteredUser = async (createdAt: number, credits = 0) => {
  const u = newUser("meter");
  const sandboxId = boatId();
  await boatControl("__extra", { id: sandboxId, state: "idle", createdAt: new Date(createdAt).toISOString() });
  await runInDurableObject(deviceStub(u), (instance, state) => {
    const device = instance as unknown as DeviceInternals;
    device.acct.orgId = u.orgId;
    device.acct.userId = u.userId;
    device.acct.deviceId = "cloud-test";
    ensureMeteringTables(state.storage.sql);
    recordSandbox(state.storage.sql, { provider: "boat", sandboxId, type: "default", createdAt, chatId: "chat-metered" });
    if (credits) addCredits(state.storage.sql, { at: createdAt, credits, reason: "test" });
  });
  await index().register({ orgId: u.orgId, userId: u.userId, deviceId: "cloud-test", sandboxes: [{ provider: "boat", sandboxId }] });
  return { u, sandboxId };
};

describe("usage reconciliation", () => {
  it("splits a sandbox into UTC days, closes ended days, and never rewrites a closed row", async () => {
    const now = Date.now();
    const today = dayKey(now);
    const createdAt = dayStart(today) - 2 * DAY - 6 * 60 * 60_000; // 18:00, three days back
    const { u, sandboxId } = await meteredUser(createdAt);
    const stub = deviceStub(u);

    const rows = await runInDurableObject(stub, async (instance, state) => {
      expect(await (instance as unknown as DeviceInternals).reconcile(now, true)).toBe(true);
      return listUsage(state.storage.sql);
    });
    const byDay = new Map(rows.map((r) => [r.day, r]));
    const first = dayKey(createdAt);
    const middle = dayKey(dayEnd(first));
    const yesterday = dayKey(dayStart(today) - 1);
    expect([...byDay.keys()]).toEqual([first, middle, yesterday, today]);
    // Windows are [day start, min(now, day end)), clamped by the provider to
    // the sandbox's life; the fake bills 1 s per wall second.
    expect(byDay.get(first)).toMatchObject({ provider: "boat", sandboxId, seconds: 6 * 3600, closed: true });
    expect(byDay.get(middle)).toMatchObject({ seconds: 86_400, closed: true });
    expect(byDay.get(yesterday)!.closed).toBe(now >= dayStart(today) + 60 * 60_000);
    expect(byDay.get(today)!.closed).toBe(false);

    // The provider's numbers change (a re-rating); closed days must not.
    await boatControl("__set", { sandboxId, rate: 2 });
    const later = await runInDurableObject(stub, async (instance, state) => {
      await (instance as unknown as DeviceInternals).reconcile(Date.now(), true);
      return listUsage(state.storage.sql);
    });
    const after = new Map(later.map((r) => [r.day, r]));
    expect(after.get(middle)).toEqual(byDay.get(middle));
    expect(after.get(today)!.seconds).toBeGreaterThanOrEqual(2 * byDay.get(today)!.seconds - 2);
  });

  it("records a failed read without losing the last figure and retries it", async () => {
    const { u, sandboxId } = await meteredUser(earlierToday(60 * 60_000));
    const stub = deviceStub(u);
    const first = await runInDurableObject(stub, async (instance, state) => {
      await (instance as unknown as DeviceInternals).reconcile(Date.now(), true);
      return listUsage(state.storage.sql);
    });
    await boatControl("__fail", { sandboxId, op: "usage", status: 503, code: "http_503" });
    const rows = await runInDurableObject(stub, async (instance, state) => {
      expect(await (instance as unknown as DeviceInternals).reconcile(Date.now(), true)).toBe(false);
      return listUsage(state.storage.sql);
    });
    expect(rows).toHaveLength(1);
    expect(rows[0]!.seconds).toBe(first[0]!.seconds); // last good figure kept
    expect(rows[0]!.reconcileError).toContain("http_503");
  });
});

describe("credits", () => {
  type Usage = { from: string; to: string; days: { day: string; credits: number }[]; credits: number; balance: number; available: boolean };

  it("serves the caller's balance and credits per day over a range", async () => {
    const now = Date.now();
    const createdAt = dayStart(dayKey(now)) - DAY; // all of yesterday, plus today so far
    const { u } = await meteredUser(createdAt, 5000);
    await runInDurableObject(deviceStub(u), (instance) => (instance as unknown as DeviceInternals).reconcile(now, true));

    const mine = (await (await call("GET", "/cloud/org1/usage", userBearer(u))).json()) as Usage;
    expect(mine.days).toHaveLength(30);
    expect(mine.to).toBe(dayKey(now));
    const yesterday = mine.days.at(-2)!;
    expect(yesterday).toEqual({ day: dayKey(createdAt), credits: 1440 }); // a minute = a credit
    expect(mine.days.slice(0, -2).every((d) => d.credits === 0)).toBe(true);
    expect(mine.credits).toBeGreaterThanOrEqual(1440);
    expect(mine.balance).toBeCloseTo(5000 - mine.credits, 0);

    const range = (await (
      await call("GET", `/cloud/org1/usage?from=${dayKey(createdAt)}&to=${dayKey(createdAt)}`, userBearer(u))
    ).json()) as Usage;
    expect(range.days).toEqual([{ day: dayKey(createdAt), credits: 1440 }]);
    expect(range.credits).toBe(1440);

    const other = (await (await call("GET", "/cloud/org1/usage", userBearer(newUser()))).json()) as Usage;
    expect(other).toMatchObject({ credits: 0, balance: 0 });
    expect((await call("GET", "/cloud/other-org/usage", userBearer(u))).status).toBe(403);
    for (const query of ["?from=2026-13-01", "?from=2026-10-05&to=2026-10-01", "?from=2020-01-01&to=2026-01-01"]) {
      expect((await call("GET", `/cloud/org1/usage${query}`, userBearer(u))).status, query).toBe(400);
    }
    expect((await call("GET", "/cloud/org1/usage", `Bearer runner:${u.userId}@org1:cloud-x`)).status).toBe(403);
  });

  it("operators grant credits with the admin token", async () => {
    const { u } = await meteredUser(Date.now());
    const grant = (body: unknown, token = "test-admin-token") =>
      call("POST", "/admin/cloud/credits", `Bearer ${token}`, body);
    expect((await grant({ orgId: u.orgId, userId: u.userId, credits: 100, reason: "x" }, "wrong")).status).toBe(401);
    expect((await grant({ orgId: u.orgId, userId: u.userId, credits: "100", reason: "x" })).status).toBe(400);
    const reply = await grant({ orgId: u.orgId, userId: u.userId, credits: 250, reason: "beta" });
    expect(reply.status).toBe(200);
    expect(((await reply.json()) as { balance: number }).balance).toBeGreaterThan(249);
  });
});

describe("operator export", () => {
  const admin = (token?: string, query = "") =>
    call("GET", `/admin/cloud/usage${query}`, token === undefined ? undefined : `Bearer ${token}`);

  it("requires the admin token and returns every user's rows, also as CSV", async () => {
    const { u, sandboxId } = await meteredUser(earlierToday(60 * 60_000));
    await runInDurableObject(deviceStub(u), (instance) => (instance as unknown as DeviceInternals).reconcile(Date.now(), true));

    expect((await admin()).status).toBe(401);
    expect((await admin("wrong-token")).status).toBe(401);
    expect((await call("GET", "/admin/cloud/usage?token=test-admin-token")).status).toBe(401);
    const reply = await admin("test-admin-token");
    expect(reply.status).toBe(200);
    const data = (await reply.json()) as {
      month: string; users: { userId: string; sandboxes: { provider: string; sandboxId: string }[] }[]; totals: { users: number };
    };
    const row = data.users.find((x) => x.userId === u.userId);
    expect(row?.sandboxes).toEqual([expect.objectContaining({ provider: "boat", sandboxId })]);
    expect(data.totals.users).toBeGreaterThanOrEqual(1);

    const csv = await (await admin("test-admin-token", "?format=csv")).text();
    expect(csv.split("\n")[0]).toBe("kind,month,orgId,userId,deviceId,provider,sandboxId,sandboxType,seconds,dollars,running,reconciledAt,closed,note");
    expect(csv).toContain(`user,${data.month},org1,${u.userId},cloud-test,boat,${sandboxId},default,`);
    expect((await admin("test-admin-token", "?month=bogus")).status).toBe(400);
  });

  it("does not exist without ADMIN_TOKEN", async () => {
    const url = new URL("https://edge.test/admin/cloud/usage");
    const reply = await handleAdminRoute(
      new Request(url, { headers: { authorization: "Bearer test-admin-token" } }),
      { ...(env as unknown as Env), ADMIN_TOKEN: undefined },
      url
    );
    expect(reply?.status).toBe(404);
  });

  it("writes a month's export to R2 once every figure is final", async () => {
    const e = env as unknown as Env;
    expect(await exportMonth(e, "2020-01", Date.now())).toBe(true);
    const stored = await env.BLOBS.get("usage/2020-01.json");
    expect(JSON.parse(await stored!.text())).toMatchObject({ month: "2020-01", closed: true });
    expect(await exportMonth(e, "2020-01", Date.now())).toBe(false); // once
    expect(await exportMonth(e, monthKey(Date.now()), Date.now())).toBe(false); // not over yet
  });
});

describe("orphans", () => {
  it("flags provider sandboxes no user owns (once they are old enough), and clears them on registration", async () => {
    const stale = boatId();
    const young = boatId();
    await boatControl("__extra", { id: stale, state: "idle", createdAt: new Date(Date.now() - 2 * DAY).toISOString() });
    await boatControl("__extra", { id: young, state: "idle", createdAt: new Date().toISOString() });
    const { sandboxId: owned } = await meteredUser(Date.now() - 2 * DAY);

    const found = await scanOrphans(env as unknown as Env, Date.now());
    expect(found).toContain(`boat/${stale}`);
    expect(found).not.toContain(`boat/${young}`);
    expect(found).not.toContain(`boat/${owned}`);
    expect(await index().orphans()).toEqual(
      expect.arrayContaining([expect.objectContaining({ provider: "boat", sandboxId: stale, state: "ready" })])
    );

    await index().register({ orgId: "org1", userId: "late", sandboxes: [{ provider: "boat", sandboxId: stale }] });
    expect((await index().orphans()).some((o) => o.sandboxId === stale)).toBe(false);
  });
});
