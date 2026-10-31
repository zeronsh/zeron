/**
 * Machines never outlive their chats, and a failed delete never waits on a
 * person: failed deletes retry on their own, and machines whose chat is gone
 * (deleted anywhere, the follow-up delete call lost) are swept.
 */
import { env, runInDurableObject } from "cloudflare:test";
import { describe, expect, it } from "vitest";
import { encodeHlc } from "../../src/registry-core";
import { cloudAccountName } from "../../src/cloud/policy";
import { boatControl, boatState, call, newUser, readySession, sessions, userBearer, waitForSession, type TestUser } from "./cloud-helpers";

const account = (u: TestUser) => env.CLOUD_ACCOUNTS!.get(env.CLOUD_ACCOUNTS!.idFromName(cloudAccountName(u.orgId, u.userId)));
// The account's alarm work, run as if the clock were `ahead` ms later.
const later = (u: TestUser, method: "retryDeletes" | "sweepOrphans", ahead: number) =>
  runInDurableObject(account(u), async (instance) => {
    await (instance as unknown as Record<string, (now: number) => Promise<void>>)[method]!(Date.now() + ahead);
  });
const gone = async (u: TestUser, chatId: string) =>
  expect.poll(async () => (await sessions(u)).some((s) => s.chatId === chatId), { timeout: 15_000, interval: 100 }).toBe(false);

describe("Cloud machine cleanup", () => {
  it("a failed delete retries on its own until the machine is gone", async () => {
    const u = newUser();
    const { chatId, sandboxId } = await readySession(u);
    // The provider refuses the stop once.
    await boatControl("__fail", { sandboxId, op: "stop", status: 403, code: "api_key_action_forbidden", action: "sandbox.stop" });
    await call("DELETE", `/cloud/${u.orgId}/sessions/${chatId}`, userBearer(u));
    expect(await waitForSession(u, chatId, "error")).toMatchObject({ failedAction: "delete" });
    const retryAt = await runInDurableObject(account(u), async (instance) => {
      const s = (instance as unknown as { sessions: Map<string, { deleteRetryAt?: number; deleteAttempts?: number }> }).sessions.get(chatId)!;
      expect(s.deleteAttempts).toBe(1);
      return s.deleteRetryAt!;
    });
    expect(retryAt).toBeGreaterThan(Date.now());

    // Not yet due: nothing happens. Due: the delete runs again, nobody asked.
    await later(u, "retryDeletes", 0);
    expect(await waitForSession(u, chatId, "error")).toBeDefined();
    await later(u, "retryDeletes", 2 * 60_000);
    await gone(u, chatId);
    expect((await boatState()).calls.some((c) => c.sandboxId === sandboxId && c.method === "DELETE")).toBe(true);
  });

  it("a machine whose chat is gone is deleted; one whose chat is live stays", async () => {
    const u = newUser();
    const orphan = await readySession(u);
    const kept = await readySession(u, undefined, { accountDeviceId: orphan.accountDeviceId, spaceId: orphan.spaceId });
    // Only the kept chat has its row (the orphan's was deleted, or never landed).
    const pushed = await call("POST", `/registry/${u.orgId}/push?device=laptop`, userBearer(u), {
      batch: `b-${crypto.randomUUID()}`,
      ops: [{ kind: "chats", id: kept.chatId, op: "upsert", set: { deviceId: kept.deviceId }, hlc: encodeHlc(Date.now(), 0, "laptop") }]
    });
    expect(pushed.status).toBe(200);

    // Within the grace period a missing row may still be on its way.
    await later(u, "sweepOrphans", 0);
    expect((await sessions(u)).some((s) => s.chatId === orphan.chatId)).toBe(true);
    // Past it, the orphan goes; the live chat's machine stays.
    await later(u, "sweepOrphans", 20 * 60_000);
    await gone(u, orphan.chatId);
    expect((await sessions(u)).find((s) => s.chatId === kept.chatId)?.state).toBe("ready");
  });
});
