import { env, runInDurableObject } from "cloudflare:test";
import { describe, expect, it } from "vitest";
import { exportMonth, handleAdminRoute, scanOrphans } from "../../src/cloud/billing";
import { CLOUD_INDEX_NAME } from "../../src/cloud/cloud-index";
import {
  ensureMeteringTables,
  listUsage,
  monthEnd,
  monthKey,
  monthStart,
  previousMonth,
  recordSandbox,
  upsertUsage
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
  reconcile(now?: number): Promise<boolean>;
};

/** A start time inside the current month (tests run at any time of day,
 * including right after midnight on the 1st). */
const earlierThisMonth = (ms: number) => Math.max(monthStart(monthKey(Date.now())), Date.now() - ms);

/** A user whose Cloud history holds one Boat sandbox created at `createdAt`
 * (the fake Boat API knows it too), registered with the billing index. */
const meteredUser = async (createdAt: number) => {
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
  });
  await index().register({ orgId: u.orgId, userId: u.userId, deviceId: "cloud-test", sandboxes: [{ provider: "boat", sandboxId }] });
  return { u, sandboxId };
};

describe("usage reconciliation", () => {
  it("splits a sandbox across month boundaries, closes ended months, and never rewrites a closed row", async () => {
    const now = Date.now();
    const thisMonth = monthKey(now);
    const prev = previousMonth(thisMonth);
    const older = previousMonth(prev);
    const createdAt = monthEnd(older) - 3 * DAY; // three days before `prev` began
    const { u, sandboxId } = await meteredUser(createdAt);
    const stub = deviceStub(u);

    const rows = await runInDurableObject(stub, async (instance, state) => {
      expect(await (instance as unknown as DeviceInternals).reconcile(now)).toBe(true);
      return listUsage(state.storage.sql);
    });
    const byMonth = new Map(rows.map((r) => [r.month, r]));
    expect([...byMonth.keys()]).toEqual([older, prev, thisMonth]);
    // Windows are [month start, min(now, month end)), clamped by the
    // provider to the sandbox's life; the fake bills 1 s per wall second.
    expect(byMonth.get(older)).toMatchObject({ provider: "boat", sandboxId, seconds: 3 * 86_400, closed: true });
    expect(byMonth.get(prev)!.seconds).toBe((monthEnd(prev) - monthStart(prev)) / 1000);
    expect(byMonth.get(prev)!.closed).toBe(now >= monthStart(thisMonth) + 60 * 60_000);
    expect(byMonth.get(thisMonth)!.closed).toBe(false);
    expect(byMonth.get(thisMonth)!.seconds).toBeGreaterThanOrEqual(Math.floor((now - monthStart(thisMonth)) / 1000));

    // The provider's numbers change (a re-rating); closed months must not.
    await boatControl("__set", { sandboxId, rate: 2 });
    const later = await runInDurableObject(stub, async (instance, state) => {
      await (instance as unknown as DeviceInternals).reconcile(Date.now());
      return listUsage(state.storage.sql);
    });
    const after = new Map(later.map((r) => [r.month, r]));
    expect(after.get(older)).toEqual(byMonth.get(older));
    expect(after.get(thisMonth)!.seconds).toBeGreaterThanOrEqual(2 * byMonth.get(thisMonth)!.seconds - 2);
  });

  it("records a failed read without losing the last figure and retries it", async () => {
    const { u, sandboxId } = await meteredUser(earlierThisMonth(DAY));
    const stub = deviceStub(u);
    const first = await runInDurableObject(stub, async (instance, state) => {
      await (instance as unknown as DeviceInternals).reconcile(Date.now());
      return listUsage(state.storage.sql);
    });
    await boatControl("__fail", { sandboxId, op: "usage", status: 503, code: "http_503" });
    const rows = await runInDurableObject(stub, async (instance, state) => {
      expect(await (instance as unknown as DeviceInternals).reconcile(Date.now())).toBe(false);
      return listUsage(state.storage.sql);
    });
    expect(rows).toHaveLength(1);
    expect(rows[0]!.seconds).toBe(first[0]!.seconds); // last good figure kept
    expect(rows[0]!.reconcileError).toContain("http_503");
  });
});

describe("usage reads", () => {
  it("serves the caller's own month only", async () => {
    const { u } = await meteredUser(earlierThisMonth(DAY));
    await runInDurableObject(deviceStub(u), (instance) => (instance as unknown as DeviceInternals).reconcile(Date.now()));

    const mine = (await (await call("GET", `/cloud/org1/usage`, userBearer(u))).json()) as {
      month: string; seconds: number; sandboxes: { provider: string }[]; closed: boolean; available: boolean;
    };
    expect(mine.month).toBe(monthKey(Date.now()));
    expect(mine.seconds).toBeGreaterThanOrEqual(0);
    expect(mine.sandboxes).toEqual([expect.objectContaining({ provider: "boat", running: true })]);
    expect(mine).toMatchObject({ closed: false, available: true });

    const other = newUser();
    const theirs = (await (await call("GET", `/cloud/org1/usage`, userBearer(other))).json()) as { seconds: number; sandboxes: unknown[] };
    expect(theirs).toMatchObject({ seconds: 0, sandboxes: [] });
    expect((await call("GET", "/cloud/other-org/usage", userBearer(u))).status).toBe(403);
    expect((await call("GET", "/cloud/org1/usage?month=2026-13", userBearer(u))).status).toBe(400);
    expect((await call("GET", "/cloud/org1/usage", `Bearer runner:${u.userId}@org1:cloud-x`)).status).toBe(403);
  });
});

describe("operator export", () => {
  const admin = (token?: string, query = "") =>
    call("GET", `/admin/cloud/usage${query}`, token === undefined ? undefined : `Bearer ${token}`);

  it("requires the admin token and returns every user's rows, also as CSV", async () => {
    const { u, sandboxId } = await meteredUser(earlierThisMonth(DAY));
    await runInDurableObject(deviceStub(u), (instance) => (instance as unknown as DeviceInternals).reconcile(Date.now()));

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
