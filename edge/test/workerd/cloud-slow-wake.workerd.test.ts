/**
 * A wake far slower than normal: the user hears so (`stage: "slow"`), a resume
 * stuck in provisioning is stopped and resumed again, and the timeline says
 * why. The fake Boat's `stallResumes` sticks the next resumes in provisioning
 * until a stop, as real Boat did for 7 to 11 minutes. The slow mark is 3 s here
 * (`CLOUD_WAKE_SLOW_SECONDS`), 45 s in production.
 */
import { env, runInDurableObject } from "cloudflare:test";
import { describe, expect, it } from "vitest";
import { listLedger } from "../../src/cloud/metering";
import { cloudAccountName } from "../../src/cloud/policy";
import { boatControl, boatState, call, newUser, readySession, runnerToken, session, userBearer, waitForSession, type TestUser } from "./cloud-helpers";

const accountStub = (u: { orgId: string; userId: string }) =>
  env.CLOUD_ACCOUNTS.get(env.CLOUD_ACCOUNTS.idFromName(cloudAccountName(u.orgId, u.userId)));

const ledger = (u: TestUser) => runInDurableObject(accountStub(u), (_instance, state) => listLedger(state.storage.sql));

const asleep = async (u: TestUser, chatId: string) => {
  expect((await call("POST", `/cloud/org1/sessions/${chatId}/sleep`, userBearer(u), {})).status).toBe(200);
  await waitForSession(u, chatId, "sleeping");
};

const callsFor = async (sandboxId: string, op: string) =>
  (await boatState()).calls.filter((c) => c.sandboxId === sandboxId && c.path.endsWith(`/${op}`));

describe("a wake far slower than normal", () => {
  it("says so, kicks a resume stuck in provisioning, comes up, and the timeline says why", async () => {
    const u = newUser();
    const s = { ...(await readySession(u)), user: u };
    await asleep(s.user, s.chatId);
    await boatControl("__set", { sandboxId: s.sandboxId, stallResumes: 1 });
    const resumesBefore = (await callsFor(s.sandboxId, "resume")).length;
    expect((await call("POST", `/cloud/org1/sessions/${s.chatId}/wake`, userBearer(s.user), {})).status).toBe(200);

    // Stuck: the session says it's slow, while still starting.
    await expect.poll(async () => (await session(s.user, s.chatId))?.stage, { timeout: 15_000, interval: 200 }).toBe("slow");
    // Kicked: stopped, then resumed again, and this time it comes up.
    await expect.poll(async () => (await callsFor(s.sandboxId, "resume")).length, { timeout: 20_000, interval: 200 }).toBe(resumesBefore + 2);
    expect((await callsFor(s.sandboxId, "stop")).length).toBeGreaterThanOrEqual(2); // the sleep's, and the kick's
    await runnerToken(s.user, s.deviceId, s.key);
    const ready = await waitForSession(s.user, s.chatId, "ready", 20_000);
    expect(ready?.stage).toBeUndefined(); // heard from: no longer slow

    const entries = await ledger(s.user);
    expect(entries.find((e) => e.event === "wake_slow")?.detail).toBe("resume still provisioning after 3 s");
    await expect
      .poll(async () => (await ledger(s.user)).some((e) => e.event === "wake_timing"), { timeout: 10_000 })
      .toBe(true);
    const timing = JSON.parse((await ledger(s.user)).findLast((e) => e.event === "wake_timing")!.detail!) as {
      at: Record<string, number>;
      slowReason?: string;
      kicks?: number;
    };
    expect(timing.slowReason).toBe("resume still provisioning after 3 s");
    expect(timing.kicks).toBe(1);
    expect(timing.at.slow).toBeGreaterThanOrEqual(0);
    expect(timing.at.kicked).toBeGreaterThan(timing.at.slow!);
  });

  it("says so while a machine that's up hasn't been heard from, and stops saying it once it is", async () => {
    const u = newUser();
    const s = { ...(await readySession(u)), user: u };
    await asleep(s.user, s.chatId);
    expect((await call("POST", `/cloud/org1/sessions/${s.chatId}/wake`, userBearer(s.user), {})).status).toBe(200);
    await expect.poll(async () => (await session(s.user, s.chatId))?.stage, { timeout: 15_000, interval: 200 }).toBe("slow");
    expect((await ledger(s.user)).find((e) => e.event === "wake_slow")?.detail).toBe(
      "machine up, engine not heard from after 3 s"
    );
    // Not kicked: the machine itself came up.
    expect((await callsFor(s.sandboxId, "stop")).length).toBe(1);
    await runnerToken(s.user, s.deviceId, s.key);
    const ready = await waitForSession(s.user, s.chatId, "ready", 20_000);
    expect(ready?.stage).toBeUndefined();
  });

  it("keeps a normal wake quiet", async () => {
    const u = newUser();
    const s = { ...(await readySession(u)), user: u };
    await asleep(s.user, s.chatId);
    expect((await call("POST", `/cloud/org1/sessions/${s.chatId}/wake`, userBearer(s.user), {})).status).toBe(200);
    await runnerToken(s.user, s.deviceId, s.key);
    await waitForSession(s.user, s.chatId, "ready", 20_000);
    expect((await ledger(s.user)).some((e) => e.event === "wake_slow")).toBe(false);
  });
});
