/**
 * Durable Cloud: choosing a machine size, a wake refused for want of disk, a
 * send recovering a session in `error`, disk levels, and a sleeping machine's
 * data expiry. Boat is the fake (boat-fake.ts); everything else is real.
 */
import { env, runDurableObjectAlarm, runInDurableObject } from "cloudflare:test";
import { describe, expect, it } from "vitest";
import { START_ENGINE_COMMAND } from "../../src/cloud/install-script";
import { listLedger, listSandboxes } from "../../src/cloud/metering";
import { cloudAccountName, FAST_STALE_MIN_MS, HEARTBEAT_STALE_MS } from "../../src/cloud/policy";
import { BOAT_IMAGE_GIB } from "../../src/cloud/providers/boat";
import {
  boatControl,
  boatState,
  call,
  createSession,
  enableCloud,
  enroll,
  enrollCodeFor,
  newUser,
  readySession,
  runnerKey,
  runnerToken,
  sandboxFor,
  session,
  status,
  userBearer,
  waitForSession,
  type SessionView,
  type TestUser
} from "./cloud-helpers";

const accountStub = (u: { orgId: string; userId: string }) =>
  env.CLOUD_ACCOUNTS.get(env.CLOUD_ACCOUNTS.idFromName(cloudAccountName(u.orgId, u.userId)));

type Internals = Record<string, unknown>;
const withSession = (u: TestUser, chatId: string, fn: (s: Internals) => void) =>
  runInDurableObject(accountStub(u), (instance) => {
    fn((instance as unknown as { sessions: Map<string, Internals> }).sessions.get(chatId)!);
  });

const DAY = 24 * 60 * 60_000;
const NEEDS_118 = "This Sandbox needs 118.3 GiB of disk but the largest 'large' machine holds 111.8 GiB. Resume or fork it as a bigger type (--type).";

const wake = (u: TestUser, chatId: string, body: unknown = {}) => call("POST", `/cloud/org1/sessions/${chatId}/wake`, userBearer(u), body);

const putToSleep = async (u: TestUser, chatId: string) => {
  expect((await call("POST", `/cloud/org1/sessions/${chatId}/sleep`, userBearer(u), {})).status).toBe(200);
  return (await waitForSession(u, chatId, "sleeping"))!;
};

const resumes = async (sandboxId: string) =>
  (await boatState()).calls.filter((c) => c.path.endsWith("/resume") && c.sandboxId === sandboxId);

const beat = async (u: TestUser, s: { deviceId: string; key: Awaited<ReturnType<typeof runnerKey>> }, body: Record<string, unknown>) =>
  call("POST", "/runner/heartbeat", `Bearer ${await runnerToken(u, s.deviceId, s.key)}`, { activeRuns: 0, clients: 0, ...body });

const gib = (n: number) => Math.round(n * 1024 ** 3);

/** What `df` reads on a machine that has added `n` GiB to its base image. */
const df = (n: number) => gib(n + BOAT_IMAGE_GIB);

/** A machine asleep whose resume Boat refuses for want of disk: blocked. */
const blockedSession = async (u: TestUser) => {
  const s = await readySession(u);
  await putToSleep(u, s.chatId);
  await boatControl("__fail", { sandboxId: s.sandboxId, op: "resume", status: 409, code: "sandbox_disk_too_small", message: NEEDS_118 });
  await wake(u, s.chatId);
  const blocked = (await waitForSession(u, s.chatId, "error"))!;
  return { ...s, blocked };
};

describe("the machine catalog and the account's default", () => {
  it("lists every size with what it holds, and a user sets their default", async () => {
    const u = newUser();
    await enableCloud(u);
    const before = (await status(u)) as unknown as { machine: string; machines: { id: string; diskGiB: number; creditMultiplier: number }[] };
    expect(before.machine).toBe("default");
    expect(before.machines.map((m) => m.id)).toEqual(["small", "default", "large", "xlarge"]);
    expect(before.machines.find((m) => m.id === "large")).toEqual({ id: "large", name: "Large", vcpu: 8, memoryGB: 16, diskGiB: 111.8, creditMultiplier: 2 });
    expect(before.machines.find((m) => m.id === "xlarge")?.creditMultiplier).toBe(5.56);

    expect((await call("PUT", "/cloud/org1/machine", userBearer(u), { machine: "huge" })).status).toBe(400);
    expect((await call("PUT", "/cloud/org1/machine", userBearer(u), {})).status).toBe(400);
    const set = await call("PUT", "/cloud/org1/machine", userBearer(u), { machine: "xlarge" });
    expect(set.status).toBe(200);
    expect(await set.json()).toMatchObject({ machine: "xlarge" });
    expect(await status(u)).toMatchObject({ machine: "xlarge" });

    // New sessions start on it.
    const { spaceId } = await enableCloud(u);
    const created = (await (await createSession(u, "on-xlarge", spaceId)).json()) as SessionView & { machine: string };
    expect(created.machine).toBe("xlarge");
    await sandboxFor(created.deviceId);
    const create = (await boatState()).calls.find((c) => c.method === "POST" && c.path === "/sandboxes" && c.headers["idempotency-key"]?.startsWith(created.deviceId));
    expect(create?.body?.type).toBe("xlarge");
  });

  it("is the user's, not a machine's: a runner can't set it, and the operator route still works", async () => {
    const u = newUser();
    const s = await readySession(u);
    const bearer = `Bearer ${await runnerToken(u, s.deviceId, s.key)}`;
    expect((await call("PUT", "/cloud/org1/machine", bearer, { machine: "xlarge" })).status).toBe(403);
    const admin = await call("POST", "/admin/cloud/machine", "Bearer test-admin-token", { orgId: u.orgId, userId: u.userId, machine: "large" });
    expect(await admin.json()).toEqual({ machine: "large" });
    expect(await status(u)).toMatchObject({ machine: "large" });
  });

  it("keeps a session's own size: the default only applies to new ones", async () => {
    const u = newUser();
    const s = await readySession(u);
    await call("PUT", "/cloud/org1/machine", userBearer(u), { machine: "xlarge" });
    expect(await session(u, s.chatId)).toMatchObject({ machine: "default" });
  });
});

describe("a send recovers a session in error", () => {
  it("wakes it when it still has a sandbox", async () => {
    const u = newUser();
    const s = await readySession(u);
    await putToSleep(u, s.chatId);
    await boatControl("__fail", { sandboxId: s.sandboxId, op: "resume", status: 400, code: "sleep_timed_out" });
    await wake(u, s.chatId);
    await waitForSession(u, s.chatId, "error");
    const before = (await resumes(s.sandboxId)).length;

    await call("POST", `/device/${s.deviceId}/nudge`, userBearer(u), { chatId: s.chatId });
    await waitForSession(u, s.chatId, "starting");
    await expect.poll(async () => (await resumes(s.sandboxId)).length).toBe(before + 1);
    await runnerToken(u, s.deviceId, s.key);
    const back = (await waitForSession(u, s.chatId, "ready"))!;
    expect(back.error).toBeUndefined();
  });

  it("doesn't leave a machine billing behind a wake that failed", async () => {
    const u = newUser();
    const s = await readySession(u);
    await putToSleep(u, s.chatId);
    await boatControl("__fail", { sandboxId: s.sandboxId, op: "resume", status: 400, code: "sleep_timed_out" });
    const stopsBefore = (await boatState()).calls.filter((c) => c.sandboxId === s.sandboxId && c.path.endsWith("/stop")).length;
    await wake(u, s.chatId);
    await waitForSession(u, s.chatId, "error");
    const stops = (await boatState()).calls.filter((c) => c.sandboxId === s.sandboxId && c.path.endsWith("/stop"));
    expect(stops.length).toBe(stopsBefore + 1);
  });

  it("leaves a failure a retry can't fix alone", async () => {
    const u = newUser();
    const s = await blockedSession(u);
    const before = (await resumes(s.sandboxId)).length;
    await call("POST", `/device/${s.deviceId}/nudge`, userBearer(u), { chatId: s.chatId });
    await new Promise((r) => setTimeout(r, 200));
    expect((await session(u, s.chatId))?.state).toBe("error");
    expect((await resumes(s.sandboxId)).length).toBe(before);
  });

  it("retries a machine that never finished provisioning, with the send's edge", async () => {
    const u = newUser();
    const { spaceId } = await enableCloud(u);
    const view = (await (await createSession(u, "half-made", spaceId)).json()) as SessionView;
    await sandboxFor(view.deviceId);
    await withSession(u, "half-made", (s) => {
      s.state = "error";
      s.error = "The Cloud engine didn't come online.";
      s.failedAction = "enable";
    });
    await call("POST", `/device/${view.deviceId}/nudge`, userBearer(u), { chatId: "half-made" });
    await new Promise((r) => setTimeout(r, 1500));
    expect(await session(u, "half-made")).toMatchObject({ state: "provisioning" });
    const code = await enrollCodeFor(view.deviceId);
    expect(code).toBeTruthy();
  });
});

describe("a wake refused for want of disk", () => {
  it("becomes a typed block, with the figure, and is never retried or upsized on its own", async () => {
    const u = newUser();
    const s = await blockedSession(u);
    expect(s.blocked).toMatchObject({ failedAction: "wake", machine: "default", wakeBlocked: { reason: "disk_too_small", neededGiB: 118.3 } });
    expect(s.blocked.error).toContain("118.3 GiB");
    // Nothing tried a bigger size meanwhile.
    expect((await resumes(s.sandboxId)).map((c) => c.body?.type)).toEqual([undefined]);
    // The figure Boat gave counts as the machine's use: over what it holds.
    expect((s.blocked as unknown as { disk: unknown }).disk).toEqual({ usedGiB: 118.3, capacityGiB: 51.2, level: "critical" });
  });

  it("refuses a size smaller than it needs with a typed 409, and wakes on one that fits", async () => {
    const u = newUser();
    const s = await blockedSession(u);
    const same = await wake(u, s.chatId);
    expect(same.status).toBe(409);
    expect(await same.json()).toMatchObject({ error: "machine_too_small", neededGiB: 118.3, capacityGiB: 51.2, machine: "default" });
    const tooSmall = await wake(u, s.chatId, { machine: "large" });
    expect(tooSmall.status).toBe(409);
    expect(await tooSmall.json()).toMatchObject({ error: "machine_too_small", neededGiB: 118.3, capacityGiB: 111.8, machine: "large" });
    expect((await call("POST", `/cloud/org1/sessions/${s.chatId}/wake`, userBearer(u), { machine: "huge" })).status).toBe(400);

    const up = await wake(u, s.chatId, { machine: "xlarge" });
    expect(up.status).toBe(200);
    const started = (await up.json()) as SessionView & { machine: string; wakeBlocked?: unknown };
    expect(started.machine).toBe("xlarge");
    expect(started.wakeBlocked).toBeUndefined();
    await expect.poll(async () => (await resumes(s.sandboxId)).at(-1)?.body).toEqual({ type: "xlarge", ttlSeconds: 7200 });
    await expect.poll(async () => (await boatState()).sandboxes.find((b) => b.id === s.sandboxId)?.state).toBe("idle");
    await runnerToken(u, s.deviceId, s.key);
    expect(await waitForSession(u, s.chatId, "ready")).toMatchObject({ machine: "xlarge" });
  });

  it("types the same refusal when it lands on the sandbox after the resume was accepted", async () => {
    const u = newUser();
    const s = await readySession(u);
    await putToSleep(u, s.chatId);
    // The resume is accepted, then the sandbox fails with Boat's text on it.
    await boatControl("__fail", { sandboxId: s.sandboxId, op: "commands", status: 500, code: "down", times: 3 });
    await boatControl("__set", { sandboxId: s.sandboxId, resumeFails: NEEDS_118 });
    await wake(u, s.chatId);
    await expect
      .poll(async () => ((await session(u, s.chatId)) as unknown as { wakeBlocked?: unknown })?.wakeBlocked)
      .toEqual({ reason: "disk_too_small", neededGiB: 118.3 });
  });

  it("wakes on a bigger size out of the error state a refused resume leaves", async () => {
    const u = newUser();
    const s = await readySession(u);
    await putToSleep(u, s.chatId);
    // Real Boat: 202 on the resume, then `error` on the sandbox, which stays
    // there until a resume on another type.
    await boatControl("__set", { sandboxId: s.sandboxId, resumeFails: NEEDS_118 });
    await wake(u, s.chatId);
    await waitForSession(u, s.chatId, "error");
    expect((await boatState()).sandboxes.find((b) => b.id === s.sandboxId)?.state).toBe("error");

    const up = await wake(u, s.chatId, { machine: "xlarge" });
    expect(up.status).toBe(200);
    await expect.poll(async () => (await resumes(s.sandboxId)).at(-1)?.body).toEqual({ type: "xlarge", ttlSeconds: 7200 });
    await expect.poll(async () => (await boatState()).sandboxes.find((b) => b.id === s.sandboxId)?.state).toBe("idle");
    await runnerToken(u, s.deviceId, s.key);
    const ready = (await waitForSession(u, s.chatId, "ready")) as unknown as { machine: string; wakeBlocked?: unknown; error?: string };
    expect(ready.machine).toBe("xlarge");
    expect(ready.wakeBlocked).toBeUndefined();
    expect(ready.error).toBeUndefined();
  });

  it("a refused shrink leaves the machine as it was, wakeable on its own size", async () => {
    const u = newUser();
    await enableCloud(u);
    await call("PUT", "/cloud/org1/machine", userBearer(u), { machine: "large" });
    const s = await readySession(u);
    await putToSleep(u, s.chatId);
    // As real Boat answers it.
    await boatControl("__fail", {
      sandboxId: s.sandboxId,
      op: "resume",
      status: 400,
      code: "type_too_small",
      message: "This sandbox uses 77.7 GB of disk, which does not fit a 'small' machine (76.0 GB). Delete data in the sandbox and let it snapshot again, or resume it as 'large' or larger."
    });
    await wake(u, s.chatId, { machine: "small" });
    await expect
      .poll(async () => ((await session(u, s.chatId)) as unknown as { wakeBlocked?: { neededGiB: number } })?.wakeBlocked?.neededGiB)
      .toBe(51.9);
    const after = (await session(u, s.chatId)) as unknown as { machine: string; state: string };
    expect(after).toMatchObject({ machine: "large", state: "sleeping" });
    // Small stays refused for what it now knows, while its own size wakes it.
    expect((await wake(u, s.chatId, { machine: "small" })).status).toBe(409);
    const plain = await wake(u, s.chatId);
    expect(plain.status).toBe(200);
    await expect.poll(async () => (await resumes(s.sandboxId)).at(-1)?.body).toEqual({ ttlSeconds: 7200 });
  });
});

describe("a sleeping session's size", () => {
  it("can be set without waking it, and the next wake moves it", async () => {
    const u = newUser();
    const s = await readySession(u);
    await putToSleep(u, s.chatId);
    const before = (await resumes(s.sandboxId)).length;
    const set = await wake(u, s.chatId, { machine: "large", wake: false });
    expect(set.status).toBe(200);
    expect(await set.json()).toMatchObject({ state: "sleeping", machine: "large" });
    expect((await resumes(s.sandboxId)).length).toBe(before);

    await call("POST", `/device/${s.deviceId}/nudge`, userBearer(u), { chatId: s.chatId });
    await expect.poll(async () => (await resumes(s.sandboxId)).at(-1)?.body).toEqual({ type: "large", ttlSeconds: 7200 });
    await expect.poll(async () => (await boatState()).sandboxes.find((b) => b.id === s.sandboxId)?.state).toBe("idle");
    await runnerToken(u, s.deviceId, s.key);
    expect(await waitForSession(u, s.chatId, "ready")).toMatchObject({ machine: "large" });
  });

  it("is refused on a machine that's awake", async () => {
    const u = newUser();
    const s = await readySession(u);
    const awake = await wake(u, s.chatId, { machine: "large" });
    expect(awake.status).toBe(409);
    expect(await awake.json()).toMatchObject({ error: "machine_awake" });
    // Its own size is no change.
    expect((await wake(u, s.chatId, { machine: "default" })).status).toBe(200);
  });
});

describe("metering follows a resize", () => {
  it("records the new size and estimates at it, while the provider's figures stay what's billed", async () => {
    const u = newUser();
    const s = await readySession(u);
    await boatControl("__set", { sandboxId: s.sandboxId, rate: 2 });
    await putToSleep(u, s.chatId);
    await wake(u, s.chatId, { machine: "xlarge" });
    await expect
      .poll(() =>
        runInDurableObject(accountStub(u), (_instance, state) => listSandboxes(state.storage.sql).find((row) => row.sandboxId === s.sandboxId)?.type)
      )
      .toBe("xlarge");
    const ledger = await runInDurableObject(accountStub(u), (_instance, state) => listLedger(state.storage.sql));
    expect(ledger.find((e) => e.event === "resize")).toMatchObject({ sandboxId: s.sandboxId, type: "xlarge" });

    // The live estimate spends at the new size, 5.56× the default's.
    await runnerToken(u, s.deviceId, s.key);
    await waitForSession(u, s.chatId, "ready");
    const spent = (minutes: number) =>
      runInDurableObject(accountStub(u), (instance) => {
        const self = instance as unknown as { sessions: Map<string, Internals>; balance(now?: number): number; acct: { lastReconcileAt?: number } };
        const now = Date.now();
        self.sessions.get(s.chatId)!.awakeSince = now - minutes * 60_000;
        self.acct.lastReconcileAt = 0;
        const after = self.balance(now);
        self.sessions.get(s.chatId)!.awakeSince = now;
        return self.balance(now) - after;
      });
    expect(await spent(10)).toBeCloseTo(55.6, 1);

    // What's billed is the provider's own figure, read for this one sandbox.
    const usage = await runInDurableObject(accountStub(u), async (instance) => {
      await (instance as unknown as { reconcile(now?: number, full?: boolean): Promise<boolean> }).reconcile(Date.now(), true);
      return (instance as unknown as { billingUsage(month: string): { sandboxes: { sandboxId: string; sandboxType: string; seconds: number }[] } }).billingUsage(
        new Date().toISOString().slice(0, 7)
      );
    });
    expect(usage.sandboxes.find((row) => row.sandboxId === s.sandboxId)).toMatchObject({ sandboxType: "xlarge" });
  });
});

describe("disk levels", () => {
  const diskOf = async (u: TestUser, chatId: string) => ((await session(u, chatId)) as unknown as { disk?: unknown })?.disk;

  it("omits the disk until the engine reports, then warns against what the size holds", async () => {
    const u = newUser();
    const s = await readySession(u);
    expect(await diskOf(u, s.chatId)).toBeUndefined();
    // An engine that predates disk reports leaves it unknown.
    await beat(u, s, {});
    expect(await diskOf(u, s.chatId)).toBeUndefined();

    const reply = await beat(u, s, { diskUsedBytes: df(10) });
    // `df`'s scale, for the engine that compares its own reading with it.
    expect(await reply.json()).toEqual({ ok: true, diskCapacityGiB: 51.2 + BOAT_IMAGE_GIB });
    expect(await diskOf(u, s.chatId)).toEqual({ usedGiB: 10, capacityGiB: 51.2, level: "ok" });
    await beat(u, s, { diskUsedBytes: df(40.7) });
    expect(await diskOf(u, s.chatId)).toMatchObject({ level: "ok" });
    await beat(u, s, { diskUsedBytes: df(41) });
    expect(await diskOf(u, s.chatId)).toMatchObject({ level: "warn" });
    await beat(u, s, { diskUsedBytes: df(46.1) });
    expect(await diskOf(u, s.chatId)).toMatchObject({ level: "critical" });
    // Nothing hydrating: a clean-up shows at once.
    await beat(u, s, { diskUsedBytes: df(12) });
    expect(await diskOf(u, s.chatId)).toMatchObject({ usedGiB: 12, level: "ok" });
    // Rejected, not stored: not a byte count.
    expect((await beat(u, s, { diskUsedBytes: -1 })).status).toBe(200);
    expect(await diskOf(u, s.chatId)).toMatchObject({ usedGiB: 12 });
  });

  it("doesn't count the base image a fresh machine already reads", async () => {
    const u = newUser();
    const s = await readySession(u);
    // Measured on a real fresh machine.
    await beat(u, s, { diskUsedBytes: gib(20.7) });
    expect(await diskOf(u, s.chatId)).toMatchObject({ level: "ok" });
    expect(((await diskOf(u, s.chatId)) as { usedGiB: number }).usedGiB).toBeLessThan(1);
    // 41 GiB of data: `df` reads 62, past what a default machine holds, and yet
    // real Boat resumed this on its own size.
    await beat(u, s, { diskUsedBytes: gib(62) });
    expect(await diskOf(u, s.chatId)).toMatchObject({ usedGiB: 41.5, level: "warn" });
  });

  it("holds the high-water mark while a resumed disk is still copying in", async () => {
    const u = newUser();
    const s = await readySession(u);
    await beat(u, s, { diskUsedBytes: df(45) });
    await putToSleep(u, s.chatId);
    await boatControl("__set", { sandboxId: s.sandboxId, hydrated: false });
    await wake(u, s.chatId);
    await runnerToken(u, s.deviceId, s.key);
    await waitForSession(u, s.chatId, "ready");

    // Right after a resume `df` reads a fraction of it.
    await beat(u, s, { diskUsedBytes: df(26) });
    expect(await diskOf(u, s.chatId)).toEqual({ usedGiB: 45, capacityGiB: 51.2, level: "warn" });
    // It still rises while copying in.
    await beat(u, s, { diskUsedBytes: df(46) });
    expect(await diskOf(u, s.chatId)).toMatchObject({ usedGiB: 46 });

    // Still not there: the provider says so, and it keeps holding.
    await withSession(u, s.chatId, (r) => {
      r.lastHydrationProbeAt = 0;
    });
    await beat(u, s, { diskUsedBytes: df(30) });
    await new Promise((r) => setTimeout(r, 100));
    expect(await diskOf(u, s.chatId)).toMatchObject({ usedGiB: 46 });

    // All there: the true figure counts from the next report.
    await boatControl("__set", { sandboxId: s.sandboxId, hydrated: true });
    await withSession(u, s.chatId, (r) => {
      r.lastHydrationProbeAt = 0;
    });
    await beat(u, s, { diskUsedBytes: df(30) });
    await expect
      .poll(async () => {
        await beat(u, s, { diskUsedBytes: df(30) });
        return ((await diskOf(u, s.chatId)) as { usedGiB: number }).usedGiB;
      })
      .toBe(30);
  });
});

describe("a sleeping machine's data expiry", () => {
  const backdateStop = async (u: TestUser, sandboxId: string, chatId: string, daysAgo: number) => {
    const at = Date.now() - daysAgo * DAY;
    await boatControl("__set", { sandboxId, stoppedAt: new Date(at).toISOString() });
    await withSession(u, chatId, (r) => {
      r.stoppedAt = at;
      r.expiryCheckAt = Date.now() - 1;
    });
    return at;
  };

  it("says when it expires, only while it sleeps", async () => {
    const u = newUser();
    const s = await readySession(u);
    expect((await session(u, s.chatId)) as unknown as { expiresAt?: number }).not.toHaveProperty("expiresAt");
    const before = Date.now();
    const asleep = (await putToSleep(u, s.chatId)) as unknown as { expiresAt: number };
    expect(asleep.expiresAt).toBeGreaterThanOrEqual(before + 14 * DAY - 1000);
    expect(asleep.expiresAt).toBeLessThanOrEqual(Date.now() + 14 * DAY + 1000);
  });

  it("resumes and re-stops a machine before it would expire, and the clock restarts", async () => {
    const u = newUser();
    const s = await readySession(u);
    await putToSleep(u, s.chatId);
    const lastActive = (await session(u, s.chatId)) as unknown as { lastActiveAt: number };
    const before = (await resumes(s.sandboxId)).length;

    // Not yet: a machine stopped 11 days ago is left alone.
    await backdateStop(u, s.sandboxId, s.chatId, 11);
    await runDurableObjectAlarm(accountStub(u));
    expect((await resumes(s.sandboxId)).length).toBe(before);
    expect((await session(u, s.chatId))?.state).toBe("sleeping");

    // Twelve days in, it comes up on its own size.
    await backdateStop(u, s.sandboxId, s.chatId, 12);
    await runDurableObjectAlarm(accountStub(u));
    await expect.poll(async () => (await resumes(s.sandboxId)).length).toBe(before + 1);
    expect((await resumes(s.sandboxId)).at(-1)?.body).toEqual({ ttlSeconds: 7200 });
    await expect.poll(async () => (await boatState()).sandboxes.find((b) => b.id === s.sandboxId)?.state).toBe("idle");

    // Up: straight back to sleep, its idle age untouched.
    await runnerToken(u, s.deviceId, s.key);
    const again = (await waitForSession(u, s.chatId, "sleeping")) as unknown as { expiresAt: number; lastActiveAt: number };
    expect(again.expiresAt).toBeGreaterThan(Date.now() + 13 * DAY);
    expect(again.lastActiveAt).toBe(lastActive.lastActiveAt);
    const ledger = await runInDurableObject(accountStub(u), (_instance, state) => listLedger(state.storage.sql));
    expect(ledger.some((e) => e.event === "refresh" && e.sandboxId === s.sandboxId)).toBe(true);
    // Nobody was waiting on it: it must not read as a wake.
    expect(ledger.some((e) => e.event === "wake_timing")).toBe(false);
  });

  it("stays up when someone sends during a refresh", async () => {
    const u = newUser();
    const s = await readySession(u);
    await putToSleep(u, s.chatId);
    await backdateStop(u, s.sandboxId, s.chatId, 13);
    await runDurableObjectAlarm(accountStub(u));
    await waitForSession(u, s.chatId, "starting");
    await call("POST", `/device/${s.deviceId}/nudge`, userBearer(u), { chatId: s.chatId });
    await expect.poll(async () => (await boatState()).sandboxes.find((b) => b.id === s.sandboxId)?.state).toBe("idle");
    await runnerToken(u, s.deviceId, s.key);
    await waitForSession(u, s.chatId, "ready");
    await new Promise((r) => setTimeout(r, 300));
    expect((await session(u, s.chatId))?.state).toBe("ready");
  });

  it("never evicts anyone, and waits for a free slot", async () => {
    const u = newUser(); // CLOUD_MAX_AWAKE is 2 in the test config
    const project = await enableCloud(u);
    const asleep = await readySession(u, "asleep", project);
    await putToSleep(u, "asleep");
    await readySession(u, "busy-1", project);
    await readySession(u, "busy-2", project);
    const before = (await resumes(asleep.sandboxId)).length;
    await backdateStop(u, asleep.sandboxId, "asleep", 13);
    await runDurableObjectAlarm(accountStub(u));
    await new Promise((r) => setTimeout(r, 100));
    expect((await resumes(asleep.sandboxId)).length).toBe(before);
    expect((await session(u, "busy-1"))?.state).toBe("ready");
    expect((await session(u, "busy-2"))?.state).toBe("ready");
    expect(((await session(u, "asleep")) as unknown as { expiresAt: number }).expiresAt).toBeGreaterThan(Date.now());
  });

  it("doesn't refresh a machine the user must act on, or spend credits it doesn't have", async () => {
    const u = newUser();
    const s = await blockedSession(u);
    const before = (await resumes(s.sandboxId)).length;
    await backdateStop(u, s.sandboxId, s.chatId, 13);
    await runDurableObjectAlarm(accountStub(u));
    await new Promise((r) => setTimeout(r, 100));
    expect((await resumes(s.sandboxId)).length).toBe(before);
    // Blocked and expiring: both are on the view for the app to say.
    expect(await session(u, s.chatId)).toMatchObject({ state: "error", wakeBlocked: { neededGiB: 118.3 } });
    expect(((await session(u, s.chatId)) as unknown as { expiresAt: number }).expiresAt).toBeGreaterThan(Date.now());

    const broke = newUser();
    const poor = await readySession(broke);
    await putToSleep(broke, poor.chatId);
    await runInDurableObject(accountStub(broke), (instance) =>
      (instance as unknown as { grantCredits(credits: number, reason: string): Promise<unknown> }).grantCredits(-1_000_000, "test")
    );
    const poorBefore = (await resumes(poor.sandboxId)).length;
    await backdateStop(broke, poor.sandboxId, poor.chatId, 13);
    await runDurableObjectAlarm(accountStub(broke));
    await new Promise((r) => setTimeout(r, 100));
    expect((await resumes(poor.sandboxId)).length).toBe(poorBefore);
  });

  it("keeps its clock when a sleep finds a machine that never came back up", async () => {
    const u = newUser();
    const s = await blockedSession(u);
    const stopped = await backdateStop(u, s.sandboxId, s.chatId, 5);
    expect((await call("POST", `/cloud/org1/sessions/${s.chatId}/sleep`, userBearer(u), {})).status).toBe(200);
    const asleep = (await waitForSession(u, s.chatId, "sleeping")) as unknown as { expiresAt: number; wakeBlocked: unknown };
    expect(asleep.expiresAt).toBe(stopped + 14 * DAY);
    // Still blocked: going back to sleep fixed nothing.
    expect(asleep.wakeBlocked).toEqual({ reason: "disk_too_small", neededGiB: 118.3 });
  });

  it("goes back to sleep unseen when a refresh can't resume", async () => {
    const u = newUser();
    const s = await readySession(u);
    await putToSleep(u, s.chatId);
    await backdateStop(u, s.sandboxId, s.chatId, 13);
    await boatControl("__fail", { sandboxId: s.sandboxId, op: "resume", status: 400, code: "nope" });
    await runDurableObjectAlarm(accountStub(u));
    await expect.poll(async () => (await resumes(s.sandboxId)).length).toBeGreaterThan(0);
    await expect
      .poll(async () => {
        const view = await session(u, s.chatId);
        return [view?.state, view?.error];
      })
      .toEqual(["sleeping", undefined]);
  });

  it("reads a session that predates this from the provider, so it gets its refresh too", async () => {
    const u = newUser();
    const s = await readySession(u);
    await putToSleep(u, s.chatId);
    await boatControl("__set", { sandboxId: s.sandboxId, stoppedAt: new Date(Date.now() - 13 * DAY).toISOString() });
    await withSession(u, s.chatId, (r) => {
      delete r.stoppedAt;
      delete r.expiryCheckAt;
      delete r.machine;
    });
    const before = (await resumes(s.sandboxId)).length;
    await runDurableObjectAlarm(accountStub(u));
    await expect.poll(async () => (await resumes(s.sandboxId)).length).toBe(before + 1);
    expect(await session(u, s.chatId)).toMatchObject({ machine: "default" });
  });
});

describe("a machine whose data is gone", () => {
  const expired = async (u: TestUser) => {
    const s = await readySession(u, undefined, await enableCloud(u));
    await putToSleep(u, s.chatId);
    await boatControl("__set", { sandboxId: s.sandboxId, dataExpiredAt: new Date().toISOString(), stoppedAt: new Date(Date.now() - 15 * DAY).toISOString() });
    await withSession(u, s.chatId, (r) => {
      r.stoppedAt = Date.now() - 15 * DAY;
      r.expiryCheckAt = Date.now() - 1;
    });
    await runDurableObjectAlarm(accountStub(u));
    await waitForSession(u, s.chatId, "data_expired");
    return s;
  };

  it("reads data_expired, and neither a send nor a wake starts anything", async () => {
    const u = newUser();
    const s = await expired(u);
    const view = (await session(u, s.chatId)) as unknown as { error: string; expiresAt: number };
    expect(view.error).toContain("files were deleted");
    expect(view.expiresAt).toBeLessThan(Date.now());
    const before = (await resumes(s.sandboxId)).length;
    await call("POST", `/device/${s.deviceId}/nudge`, userBearer(u), { chatId: s.chatId });
    await new Promise((r) => setTimeout(r, 200));
    expect((await resumes(s.sandboxId)).length).toBe(before);
    const refused = await wake(u, s.chatId);
    expect(refused.status).toBe(409);
    expect(await refused.json()).toMatchObject({ error: "data_expired" });
  });

  it("never reaches an already-shipped engine as a state it can't parse", async () => {
    // Every state a shipped engine's closed `CloudState` knows (older than this
    // edge; one it doesn't know fails its whole session list).
    const SHIPPED = ["off", "provisioning", "starting", "ready", "sleeping", "stopping", "deleting", "error"];
    const u = newUser();
    const s = await expired(u);
    const bearer = userBearer(u);

    const listed = (await (await call("GET", "/cloud/org1/sessions", bearer)).json()) as { sessions: (SessionView & { dataExpired?: boolean })[] };
    const old = listed.sessions.find((x) => x.chatId === s.chatId)!;
    expect(SHIPPED).toContain(old.state);
    expect(old).toMatchObject({ state: "error", failedAction: "wake", dataExpired: true });
    expect(old.error).toContain("files were deleted");

    // A session comes back from every other route too.
    const again = await createSession(u, s.chatId, s.spaceId);
    expect(SHIPPED).toContain(((await again.json()) as SessionView).state);
    const slept = await call("POST", `/cloud/org1/sessions/${s.chatId}/sleep`, bearer, {});
    expect(SHIPPED).toContain(((await slept.json()) as SessionView).state);

    // Its Retry is a wake, which says plainly why not.
    const retried = await call("POST", `/cloud/org1/sessions/${s.chatId}/wake`, bearer, {});
    expect(retried.status).toBe(409);
    expect(await retried.json()).toMatchObject({ error: "data_expired" });

    // A current client asks for the state itself, and gets no compatibility field.
    const current = (await session(u, s.chatId)) as unknown as { state: string; dataExpired?: boolean; failedAction?: string };
    expect(current.state).toBe("data_expired");
    expect(current.dataExpired).toBeUndefined();
    expect(current.failedAction).toBeUndefined();
    const named = await call("POST", `/cloud/org1/sessions/${s.chatId}/sleep`, bearer, {}, { "x-zeron-cloud-states": "something_later, data_expired" });
    expect(((await named.json()) as SessionView).state).toBe("data_expired");
  });

  it("leaves every other session as it is for an older client", async () => {
    const u = newUser();
    const s = await readySession(u);
    const listed = (await (await call("GET", "/cloud/org1/sessions", userBearer(u))).json()) as { sessions: SessionView[] };
    const old = listed.sessions.find((x) => x.chatId === s.chatId) as SessionView & { dataExpired?: boolean };
    expect(old.state).toBe("ready");
    expect(old.dataExpired).toBeUndefined();
  });

  it("types Boat's refusal of a resume the same way", async () => {
    const u = newUser();
    const s = await readySession(u);
    await putToSleep(u, s.chatId);
    await boatControl("__set", { sandboxId: s.sandboxId, dataExpiredAt: new Date().toISOString() });
    await wake(u, s.chatId);
    await waitForSession(u, s.chatId, "data_expired");
  });

  it("starts a fresh machine on the same repository, branch and device", async () => {
    const u = newUser();
    const s = await expired(u);
    const created = (await boatState()).sandboxes.find((b) => b.id === s.sandboxId)!;
    const fresh = await call("POST", `/cloud/org1/sessions/${s.chatId}/retry`, userBearer(u), { machine: "large" });
    expect(fresh.status).toBe(200);
    expect(await fresh.json()).toMatchObject({ state: "provisioning", deviceId: s.deviceId, machine: "large" });

    let next: Awaited<ReturnType<typeof boatState>>["sandboxes"][number] | undefined;
    await expect
      .poll(async () => {
        const state = await boatState();
        next = state.sandboxes.find((b) => b.id !== s.sandboxId && b.env.ZERON_RUNNER_ENROLL?.includes(`.${s.deviceId}.`));
        return next !== undefined;
      })
      .toBe(true);
    expect(next!.env.ZERON_CLOUD_REPO).toBe(created.env.ZERON_CLOUD_REPO);
    expect(next!.env.ZERON_CLOUD_BRANCH).toBe(created.env.ZERON_CLOUD_BRANCH);
    expect(next!.env.ZERON_CLOUD_CHAT).toBe(s.chatId);
    // The dead one is dropped.
    await expect.poll(async () => (await boatState()).sandboxes.find((b) => b.id === s.sandboxId)?.deleted).toBe(true);

    // A new disk has no key: the same device enrolls a new one.
    const key = await runnerKey();
    const code = next!.env.ZERON_RUNNER_ENROLL!.split(".").at(-1)!;
    expect((await enroll(u, s.deviceId, code, key.publicKey)).status).toBe(200);
    await expect.poll(async () => (await session(u, s.chatId))?.state).toBe("ready");
    expect(await session(u, s.chatId)).toMatchObject({ machine: "large", sandbox: { id: next!.id } });
    expect(await session(u, s.chatId)).not.toHaveProperty("expiresAt");
  });
});

describe("a machine that goes dark during a turn", () => {
  const goDark = (u: TestUser, chatId: string) =>
    withSession(u, chatId, (r) => {
      r.lastHeartbeatAt = Date.now() - 20 * 60_000;
      r.lastActiveAt = Date.now() - 25 * 60_000;
      r.idleCheckAt = Date.now() - 1;
    });

  it("is brought back, so its boot recovery picks the turn up", async () => {
    const u = newUser();
    const s = await readySession(u);
    await beat(u, s, { activeRuns: 1 });
    // The provider stopped it behind our back.
    await boatControl("__set", { sandboxId: s.sandboxId, state: "archived" });
    await goDark(u, s.chatId);
    const before = (await resumes(s.sandboxId)).length;
    await runDurableObjectAlarm(accountStub(u));
    await expect.poll(async () => (await resumes(s.sandboxId)).length).toBe(before + 1);
    await waitForSession(u, s.chatId, "starting");
    const ledger = await runInDurableObject(accountStub(u), (_instance, state) => listLedger(state.storage.sql));
    expect(ledger.some((e) => e.detail === "went dark during a turn; recovering it")).toBe(true);
  });

  it("is brought back when it dies right after a send, before any heartbeat reported the turn", async () => {
    const u = newUser();
    const s = await readySession(u);
    await beat(u, s, { activeRuns: 0 });
    // Seen on a real machine: stopped seconds after the send.
    await call("POST", `/device/${s.deviceId}/nudge`, userBearer(u), { chatId: s.chatId });
    await boatControl("__set", { sandboxId: s.sandboxId, state: "archived" });
    const before = Date.now();
    await withSession(u, s.chatId, (r) => {
      expect(r.idleCheckAt!).toBeLessThanOrEqual(before + HEARTBEAT_STALE_MS);
      r.lastHeartbeatAt = r.lastNudgeAt! - HEARTBEAT_STALE_MS;
      r.idleCheckAt = Date.now() - 1;
    });
    const resumed = (await resumes(s.sandboxId)).length;
    await runDurableObjectAlarm(accountStub(u));
    await expect.poll(async () => (await resumes(s.sandboxId)).length).toBe(resumed + 1);
  });

  it("is looked at as soon as its heartbeats are late, not an idle window on", async () => {
    const u = newUser();
    const s = await readySession(u);
    const before = Date.now();
    await beat(u, s, { activeRuns: 1 });
    await withSession(u, s.chatId, (r) => {
      expect(r.idleCheckAt!).toBeLessThanOrEqual(Date.now() + HEARTBEAT_STALE_MS);
      expect(r.idleCheckAt!).toBeGreaterThanOrEqual(before + HEARTBEAT_STALE_MS);
    });
    // Just late: it was active a moment before it went quiet.
    await boatControl("__set", { sandboxId: s.sandboxId, state: "archived" });
    await withSession(u, s.chatId, (r) => {
      r.lastHeartbeatAt = Date.now() - HEARTBEAT_STALE_MS;
      r.lastActiveAt = Date.now() - HEARTBEAT_STALE_MS;
      r.idleCheckAt = Date.now() - 1;
    });
    const resumed = (await resumes(s.sandboxId)).length;
    await runDurableObjectAlarm(accountStub(u));
    await expect.poll(async () => (await resumes(s.sandboxId)).length).toBe(resumed + 1);
  });

  it("starts its engine as any wake does, and its timeline shows no send behind it", async () => {
    const u = newUser();
    const s = await readySession(u);
    await beat(u, s, { activeRuns: 1 });
    await boatControl("__set", { sandboxId: s.sandboxId, state: "archived" });
    await goDark(u, s.chatId);
    await runDurableObjectAlarm(accountStub(u));
    await expect
      .poll(async () => {
        const calls = (await boatState()).calls.filter((c) => c.sandboxId === s.sandboxId);
        const resume = calls.map((c) => c.path.endsWith("/resume")).lastIndexOf(true);
        return resume >= 0 && calls.slice(resume).some((c) => c.body?.command === START_ENGINE_COMMAND);
      })
      .toBe(true);
    await runnerToken(u, s.deviceId, s.key);
    await waitForSession(u, s.chatId, "ready");
    const ledger = await runInDurableObject(accountStub(u), (_instance, state) => listLedger(state.storage.sql));
    const timing = JSON.parse(ledger.findLast((e) => e.event === "wake_timing")!.detail!) as { at: Record<string, number> };
    expect(timing.at.resumed).toBeGreaterThanOrEqual(0);
    expect(timing.at.online).toBeGreaterThanOrEqual(0);
    expect(timing.at).not.toHaveProperty("sent");
  });

  it("is looked at three of its fast beats after the last, when it says it beats fast", async () => {
    const u = newUser();
    const s = await readySession(u);
    const before = Date.now();
    await beat(u, s, { activeRuns: 1, everySecs: 15 });
    await withSession(u, s.chatId, (r) => {
      expect(r.beatEveryMs).toBe(15_000);
      expect(r.idleCheckAt!).toBeGreaterThanOrEqual(before + FAST_STALE_MIN_MS);
      expect(r.idleCheckAt!).toBeLessThanOrEqual(Date.now() + FAST_STALE_MIN_MS);
    });
    // Hard-stopped mid-turn: 45 s without a beat is enough.
    await boatControl("__set", { sandboxId: s.sandboxId, state: "archived" });
    await withSession(u, s.chatId, (r) => {
      r.lastHeartbeatAt = Date.now() - FAST_STALE_MIN_MS - 1;
      r.idleCheckAt = Date.now() - 1;
    });
    const calls = (await boatState()).calls.length;
    const resumed = (await resumes(s.sandboxId)).length;
    await runDurableObjectAlarm(accountStub(u));
    await expect.poll(async () => (await resumes(s.sandboxId)).length).toBe(resumed + 1);
    // Already stopped: nothing was sent to a machine that can't answer.
    const since = (await boatState()).calls.slice(calls);
    expect(since.filter((c) => c.sandboxId === s.sandboxId && c.path.endsWith("/commands") && since.indexOf(c) < since.findIndex((r) => r.path.endsWith("/resume")))).toEqual([]);
  });

  it("isn't dark 45 s after a beat at the idle pace, or from an engine that doesn't say", async () => {
    for (const body of [{ activeRuns: 1, everySecs: 60 }, { activeRuns: 1 }]) {
      const u = newUser();
      const s = await readySession(u);
      await beat(u, s, body);
      await withSession(u, s.chatId, (r) => {
        expect(r.idleCheckAt!).toBeGreaterThanOrEqual(Date.now() + HEARTBEAT_STALE_MS - 5_000);
        r.lastHeartbeatAt = Date.now() - FAST_STALE_MIN_MS - 1;
        r.idleCheckAt = Date.now() - 1;
      });
      const resumed = (await resumes(s.sandboxId)).length;
      await runDurableObjectAlarm(accountStub(u));
      expect((await session(u, s.chatId))?.state).toBe("ready");
      expect((await resumes(s.sandboxId)).length).toBe(resumed);
    }
  });

  it("records once, and shows, that its engine rebuilt a damaged chat store", async () => {
    const u = newUser();
    const s = await readySession(u);
    const at = Date.now() - 5_000;
    await beat(u, s, { activeRuns: 0, storeRecoveredAt: at });
    await beat(u, s, { activeRuns: 0, storeRecoveredAt: at });
    expect(await session(u, s.chatId)).toMatchObject({ storeRecoveredAt: at });
    const ledger = await runInDurableObject(accountStub(u), (_instance, state) => listLedger(state.storage.sql));
    expect(ledger.filter((e) => e.event === "store_recovered")).toHaveLength(1);
  });

  it("forgets a fast cadence once asleep", async () => {
    const u = newUser();
    const s = await readySession(u);
    await beat(u, s, { activeRuns: 0, everySecs: 15 });
    await putToSleep(u, s.chatId);
    await withSession(u, s.chatId, (r) => expect(r.beatEveryMs).toBeUndefined());
  });

  it("just sleeps when it had nothing running", async () => {
    const u = newUser();
    const s = await readySession(u);
    await beat(u, s, { activeRuns: 0 });
    await goDark(u, s.chatId);
    const before = (await resumes(s.sandboxId)).length;
    await runDurableObjectAlarm(accountStub(u));
    await waitForSession(u, s.chatId, "sleeping");
    await new Promise((r) => setTimeout(r, 100));
    expect((await resumes(s.sandboxId)).length).toBe(before);
  });

  it("ends in a clear error when the provider lost the machine", async () => {
    const u = newUser();
    const s = await readySession(u);
    await beat(u, s, { activeRuns: 1 });
    await boatControl("__set", { sandboxId: s.sandboxId, state: "error" });
    await goDark(u, s.chatId);
    await runDurableObjectAlarm(accountStub(u));
    const failed = (await waitForSession(u, s.chatId, "error"))!;
    expect(failed.error).toBeTruthy();
  });
});
