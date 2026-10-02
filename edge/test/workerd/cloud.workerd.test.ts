import { SELF, env, introspectWorkflow, runDurableObjectAlarm, runInDurableObject } from "cloudflare:test";
import { describe, expect, it } from "vitest";
import { authenticate } from "../../src/auth";
import { handleAuthRoute } from "../../src/auth-routes";
import { listLedger, listSandboxes, listUsage } from "../../src/cloud/metering";
import { cloudAccountName } from "../../src/cloud/policy";
import type { Env } from "../../src/env";
import {
  boatControl,
  boatState,
  call,
  createSession,
  enableCloud,
  enroll,
  enrollCodeFor,
  newUser,
  REPO,
  readySession,
  requestToken,
  runnerKey,
  runnerToken,
  sandboxFor,
  session,
  status,
  userBearer,
  waitForSession,
  waitForState,
  type SessionView,
  type TestUser
} from "./cloud-helpers";

const accountStub = (u: { orgId: string; userId: string }) =>
  env.CLOUD_ACCOUNTS.get(env.CLOUD_ACCOUNTS.idFromName(cloudAccountName(u.orgId, u.userId)));

const vaultCalls = async (method: string) => (await env.VAULT.calls()).filter((c) => c.method === method);

type SessionInternals = Record<string, number | string | boolean | undefined>;
/** Mutate one session's in-memory record inside its account DO. */
const withSession = (u: TestUser, chatId: string, fn: (s: SessionInternals) => void) =>
  runInDurableObject(accountStub(u), (instance) => {
    fn((instance as unknown as { sessions: Map<string, SessionInternals> }).sessions.get(chatId)!);
  });

const registryRows = async (u: TestUser) =>
  ((await (await call("GET", `/registry/${u.orgId}/rows`, userBearer(u))).json()) as {
    rows: { kind: string; id: string; deleted: boolean; fields: Record<string, unknown> }[];
  }).rows;

describe("account", () => {
  it("reports off + available for a new user and refuses another org", async () => {
    const u = newUser();
    expect(await status(u)).toEqual({ state: "off", awakeSessions: 0, maxAwakeSessions: 2, available: true });
    expect((await call("GET", "/cloud/other-org", userBearer(u))).status).toBe(403);
    expect((await call("GET", "/cloud/org1")).status).toBe(401);
  });

  it("enables instantly (no machine), idempotently, and lists no Cloud device", async () => {
    const u = newUser();
    const first = (await (await call("POST", "/cloud/org1/enable", userBearer(u), {})).json()) as { state: string; deviceId: string };
    expect(first.state).toBe("ready");
    expect(first.deviceId).toMatch(/^cloud-[0-9a-f-]{36}$/);
    const second = (await (await call("POST", "/cloud/org1/enable", userBearer(u), {})).json()) as { deviceId: string };
    expect(second.deviceId).toBe(first.deviceId);
    // Enabling provisions nothing: machines are per session.
    expect((await boatState()).sandboxes.some((s) => s.env.ZERON_RUNNER_ENROLL?.includes(u.userId))).toBe(false);
    // Cloud is a checkout option, not a device: no registry row.
    expect((await registryRows(u)).some((r) => r.kind === "devices" && r.id === first.deviceId)).toBe(false);
  });

  it("starts sessions only while Cloud is on, on a valid GitHub repository", async () => {
    const u = newUser();
    const early = await createSession(u, "chat-early", "sp-1");
    expect(early.status).toBe(409);
    expect(await early.json()).toMatchObject({ error: "cloud_off" });
    await call("POST", "/cloud/org1/enable", userBearer(u), {});
    const bad = [
      { ...REPO, cloneUrl: "git@github.com:acme/app" },
      { ...REPO, fullName: "../escape" },
      { ...REPO, fullName: "acme/.." },
      { ...REPO, defaultBranch: "bad..branch" },
      { fullName: "acme/app" }
    ];
    for (const [ix, repo] of bad.entries()) {
      expect((await createSession(u, `chat-bad-${ix}`, "sp-1", undefined, repo)).status, JSON.stringify(repo)).toBe(400);
    }
    const created = (await (await createSession(u, "chat-ok", "sp-1")).json()) as SessionView;
    expect(created).toMatchObject({ chatId: "chat-ok", spaceId: "sp-1", repo: "acme/app", path: "/home/user/app" });
  });
});

describe("sessions get their own machines", () => {
  it("provisions a noEnv sandbox per chat with the project, idempotently per chat", async () => {
    const u = newUser();
    const { accountDeviceId, spaceId } = await enableCloud(u);

    const first = (await (await createSession(u, "chat-a", spaceId)).json()) as SessionView;
    expect(first).toMatchObject({ chatId: "chat-a", spaceId, state: "provisioning" });
    expect(first.deviceId).toMatch(/^cloud-[0-9a-f-]{36}$/);
    expect(first.deviceId).not.toBe(accountDeviceId);
    const again = (await (await createSession(u, "chat-a", spaceId)).json()) as SessionView;
    expect(again.deviceId).toBe(first.deviceId);

    const sandbox = await sandboxFor(first.deviceId);
    const creates = (await boatState()).calls.filter(
      (c) => c.method === "POST" && c.path === "/sandboxes" && c.headers["idempotency-key"]?.startsWith(first.deviceId)
    );
    expect(creates).toHaveLength(1);
    expect(creates[0]!.headers["idempotency-key"]).toMatch(new RegExp(`^${first.deviceId}-g\\d+$`));
    expect(creates[0]!.body).toMatchObject({ noEnv: true, ttlSeconds: 7200, type: "default" });
    expect(sandbox.env).toMatchObject({
      ZERON_EDGE_URL: "https://edge.test",
      ZERON_DEVICE_NAME: "Cloud session",
      ZERON_DEVICE_PLATFORM: "cloud",
      ZERON_CLOUD_ACCOUNT: accountDeviceId,
      ZERON_CLOUD_CHAT: "chat-a",
      ZERON_CLOUD_REPO: "https://github.com/acme/app.git",
      ZERON_CLOUD_PATH: "/home/user/app",
      ZERON_CLOUD_BRANCH: "main"
    });
    expect(sandbox.env.ZERON_RUNNER_ENROLL).toMatch(new RegExp(`^org1\\.${u.userId}\\.${first.deviceId}\\.[A-Za-z0-9_-]{43}$`));
    await expect
      .poll(async () => (await boatState()).calls.some((c) => c.sandboxId === sandbox.id && c.path.endsWith("/commands")))
      .toBe(true);
    const install = (await boatState()).calls.find((c) => c.sandboxId === sandbox.id && c.path.endsWith("/commands"))!;
    expect(install.body!.command).toContain(`ZERON_RUNNER_ENROLL='${sandbox.env.ZERON_RUNNER_ENROLL}'`);
    expect(install.body!.command).toContain(`ZERON_CLOUD_CHAT='chat-a'`);
    expect(install.body!.timeoutSeconds).toBe(600);
  });

  it("queues a send made while the session's machine is still booting", async () => {
    const u = newUser();
    const { spaceId } = await enableCloud(u);
    const session = (await (await createSession(u, "chat-early", spaceId)).json()) as SessionView;
    // No engine has connected yet: the nudge must still queue (it replays
    // when the runner joins), not 404 on an unclaimed room.
    const nudge = await call("POST", `/device/${session.deviceId}/nudge`, userBearer(u), { chatId: "chat-early" });
    expect(nudge.status).toBe(200);
    expect(await nudge.json()).toEqual({ delivered: false, queued: true });
    // The room is this user's: nobody else can queue into it.
    const other = newUser();
    expect((await call("POST", `/device/${session.deviceId}/nudge`, userBearer(other), { chatId: "chat-early" })).status).toBe(403);
  });

  it("starts a session from the branch picked in the composer", async () => {
    const u = newUser();
    const { spaceId } = await enableCloud(u);
    expect((await createSession(u, "chat-b", spaceId, "bad..branch")).status).toBe(400);
    const session = (await (await createSession(u, "chat-b", spaceId, "feature/x")).json()) as SessionView;
    expect((await sandboxFor(session.deviceId)).env.ZERON_CLOUD_BRANCH).toBe("feature/x");
  });

  it("gives two chats two machines, two devices and two keys", async () => {
    const u = newUser();
    const project = await enableCloud(u);
    const a = await readySession(u, "chat-a", project);
    const b = await readySession(u, "chat-b", project);
    expect(b.deviceId).not.toBe(a.deviceId);
    expect(b.sandboxId).not.toBe(a.sandboxId);
    // Each runner can only mint tokens for its own device.
    expect((await requestToken(u, b.deviceId, a.key)).status).toBe(401);
    const tokenA = await runnerToken(u, a.deviceId, a.key);
    const ws = { upgrade: "websocket" };
    expect((await call("GET", `/device/${b.deviceId}/ws?role=host`, `Bearer ${tokenA}`, undefined, ws)).status).toBe(403);
    const own = await call("GET", `/device/${a.deviceId}/ws?role=host`, `Bearer ${tokenA}`, undefined, ws);
    expect(own.status).toBe(101);
    own.webSocket!.accept();
    own.webSocket!.close();
    // Sleeping one leaves the other running.
    expect((await call("POST", "/cloud/org1/sessions/chat-a/sleep", userBearer(u), {})).status).toBe(200);
    await waitForSession(u, "chat-a", "sleeping");
    expect((await session(u, "chat-b"))?.state).toBe("ready");
    expect((await boatState()).sandboxes.find((s) => s.id === b.sandboxId)?.state).not.toBe("archived");
  });
});

describe("runner enrollment", () => {
  it("accepts the code once, binds the key, enrolls it under the account with the vault, and marks the session ready", async () => {
    const u = newUser();
    const { accountDeviceId, spaceId } = await enableCloud(u);
    const { deviceId } = (await (await createSession(u, "chat-e", spaceId)).json()) as SessionView;
    const code = await enrollCodeFor(deviceId);
    const key = await runnerKey();
    const other = await runnerKey();

    const bad = await enroll(u, deviceId, "A".repeat(43), key.publicKey);
    expect(bad.status).toBe(403);
    expect(await bad.json()).toMatchObject({ error: "bad_code" });
    expect((await enroll(u, deviceId, code, "too-short")).status).toBe(400);
    expect((await enroll(newUser(), deviceId, code, key.publicKey)).status).toBe(404);
    expect((await enroll(u, accountDeviceId, code, key.publicKey)).status).toBe(404); // the logical device has no machine

    expect((await enroll(u, deviceId, code, key.publicKey)).status).toBe(200);
    await waitForSession(u, "chat-e", "ready");

    const reused = await enroll(u, deviceId, code, other.publicKey);
    expect(reused.status).toBe(409);
    expect(await reused.json()).toMatchObject({ error: "code_used" });
    expect((await enroll(u, deviceId, code, key.publicKey)).status).toBe(200);

    const enrolls = await vaultCalls("enrollDevice");
    expect(enrolls.find((c) => (c.args[1] as { deviceId: string }).deviceId === deviceId)?.args).toEqual([
      { userId: u.userId, orgId: "org1", kind: "user" },
      { deviceId, kind: "cloud", publicKey: key.publicKey, parentId: accountDeviceId }
    ]);
  });

  it("keeps the code valid when the vault is down so the engine can retry", async () => {
    const u = newUser("vault-down");
    const { spaceId } = await enableCloud(u);
    const { deviceId } = (await (await createSession(u, "chat-v", spaceId)).json()) as SessionView;
    const code = await enrollCodeFor(deviceId);
    const key = await runnerKey();
    const reply = await enroll(u, deviceId, code, key.publicKey);
    expect(reply.status).toBe(503);
    expect(await reply.json()).toMatchObject({ error: "vault_unavailable" });
    expect((await session(u, "chat-v"))?.state).toBe("provisioning");
    await withSession(u, "chat-v", (s) => {
      expect(s.enrollHash).toEqual(expect.any(String));
    });
  });
});

describe("runner tokens", () => {
  it("mints a runner JWT naming the session device and its account, and rejects bad, stale and replayed ones", async () => {
    const u = newUser();
    const { deviceId, accountDeviceId, key } = await readySession(u);
    const ts = Date.now();
    const good = await requestToken(u, deviceId, key, ts);
    expect(good.status).toBe(200);
    const { accessToken, expiresAt } = (await good.json()) as { accessToken: string; expiresAt: number };
    expect(expiresAt - ts).toBeGreaterThan(3_500_000);
    const verified = await authenticate(env as unknown as Env, new Request("https://edge.test/x", { headers: { authorization: `Bearer ${accessToken}` } }));
    expect(verified).toEqual({ userId: u.userId, orgId: "org1", deviceId, accountDeviceId, kind: "runner" });

    const replay = await requestToken(u, deviceId, key, ts);
    expect(replay.status).toBe(401);
    expect(await replay.json()).toMatchObject({ error: "replay" });
    expect((await requestToken(u, deviceId, key, ts - 1)).status).toBe(401);
    const stale = await requestToken(u, deviceId, key, Date.now() - 5 * 60_000);
    expect(await stale.json()).toMatchObject({ error: "stale" });
    const impostor = await runnerKey();
    const forged = await requestToken(u, deviceId, impostor, Date.now() + 1);
    expect(forged.status).toBe(401);
    expect(await forged.json()).toMatchObject({ error: "bad_signature" });
    expect((await call("POST", "/runner/token", undefined, { ...u, deviceId: "laptop-1", ts, sig: "x" })).status).toBe(400);
    expect((await call("POST", "/runner/token", undefined, { ...u, deviceId, ts: "now", sig: "x" })).status).toBe(400);
  });

  it("is refused on /cloud and vault admin, but grants for itself and reads its project's space", async () => {
    const u = newUser();
    const { deviceId, spaceId, accountDeviceId, key } = await readySession(u);
    const bearer = `Bearer ${await runnerToken(u, deviceId, key)}`;

    expect((await call("GET", "/cloud/org1", bearer)).status).toBe(403);
    expect((await call("POST", "/cloud/org1/sessions", bearer, { chatId: "x", spaceId })).status).toBe(403);
    expect((await call("GET", "/cloud/org1/usage", bearer)).status).toBe(403);
    expect((await call("GET", "/vault/org1", bearer)).status).toBe(403);
    expect((await call("GET", "/vault/org1/github/repos", bearer)).status).toBe(403);
    expect((await call("GET", "/vault/org1/github/branches?repo=acme/app", bearer)).status).toBe(403);
    expect((await call("PUT", "/vault/org1/credentials/codex", bearer, { material: { key: "x" }, authorizedDevices: [] })).status).toBe(403);

    const grant = { provider: "codex", deviceId, ts: Date.now(), sig: "c2ln" };
    expect((await call("POST", "/vault/org1/grant", bearer, grant)).status).toBe(200);
    expect((await call("POST", "/vault/org1/grant", bearer, { ...grant, deviceId: "laptop-1" })).status).toBe(403);
    expect((await vaultCalls("grant")).at(-1)!.args[0]).toEqual({ userId: u.userId, orgId: "org1", kind: "runner", deviceId });

    // The chat it hosts belongs to a laptop's project: the session reads
    // that project's row, but not the laptop's other projects.
    const hlc = (n: number) => `${String(Date.now()).padStart(13, "0")}-${String(n).padStart(6, "0")}-laptop`;
    const pushed = await call("POST", "/registry/org1/push?device=laptop", userBearer(u), {
      batch: `b-${crypto.randomUUID()}`,
      ops: [
        { kind: "spaces", id: spaceId, op: "upsert", set: { deviceId: "laptop-1", path: "/Users/me/app" }, hlc: hlc(1) },
        { kind: "spaces", id: "laptop-space", op: "upsert", set: { deviceId: "laptop-1", path: "/Users/me/secret" }, hlc: hlc(2) },
        { kind: "chats", id: "chat-on-cloud", op: "upsert", set: { deviceId, spaceId }, hlc: hlc(3) }
      ]
    });
    expect(pushed.status).toBe(200);
    const seen = ((await (await call("GET", "/registry/org1/rows", bearer)).json()) as { rows: { kind: string; id: string }[] }).rows;
    expect(seen.some((r) => r.kind === "spaces" && r.id === spaceId)).toBe(true);
    expect(seen.some((r) => r.kind === "spaces" && r.id === "laptop-space")).toBe(false);
    expect(seen.some((r) => r.kind === "devices" && r.id === accountDeviceId)).toBe(false);

    const orgs = await handleAuthRoute(
      new Request("https://edge.test/auth/orgs", { headers: { authorization: bearer } }),
      { ...(env as unknown as Env), WORKOS_API_KEY: "sk_test" },
      new URL("https://edge.test/auth/orgs")
    );
    expect(orgs?.status).toBe(403);
  });
});

describe("idle sleep, waking on sends and the awake cap", () => {
  it("extends the TTL while busy, sleeps after the idle window, never wakes on a dial, and wakes on its owner's send", async () => {
    const u = newUser();
    const { chatId, deviceId, key, sandboxId } = await readySession(u);
    const bearer = `Bearer ${await runnerToken(u, deviceId, key)}`;

    expect((await call("POST", "/runner/heartbeat", userBearer(u), { activeRuns: 0, clients: 0 })).status).toBe(403);
    expect((await call("POST", "/runner/heartbeat", bearer, { activeRuns: -1, clients: 0 })).status).toBe(400);

    await withSession(u, chatId, (s) => {
      s.lastTtlExtendAt = Date.now() - 11 * 60_000;
    });
    expect((await call("POST", "/runner/heartbeat", bearer, { activeRuns: 1, clients: 1 })).status).toBe(200);
    await expect
      .poll(async () => (await boatState()).calls.some((c) => c.method === "PATCH" && c.sandboxId === sandboxId))
      .toBe(true);
    expect((await boatState()).calls.find((c) => c.method === "PATCH" && c.sandboxId === sandboxId)!.body).toEqual({ ttlSeconds: 7200 });
    expect((await call("POST", "/runner/heartbeat", bearer, { activeRuns: 2, clients: 1 })).status).toBe(200);
    expect((await boatState()).calls.filter((c) => c.method === "PATCH" && c.sandboxId === sandboxId)).toHaveLength(1);
    const backdate = () =>
      withSession(u, chatId, (s) => {
        s.lastActiveAt = Date.now() - 25 * 60_000;
        s.idleCheckAt = Date.now() - 1;
      });
    await backdate();
    await runDurableObjectAlarm(accountStub(u));
    expect((await session(u, chatId))?.state).toBe("ready"); // busy

    expect((await call("POST", "/runner/heartbeat", bearer, { activeRuns: 0, clients: 0 })).status).toBe(200);
    await backdate();
    await runDurableObjectAlarm(accountStub(u));
    await waitForSession(u, chatId, "sleeping");
    expect((await boatState()).sandboxes.find((s) => s.id === sandboxId)?.state).toBe("archived");

    // Viewing (a client dial) never wakes a session.
    await call("GET", `/device/${deviceId}/ws?role=client`, userBearer(u), undefined, { upgrade: "websocket" });
    await new Promise((r) => setTimeout(r, 200));
    expect((await session(u, chatId))?.state).toBe("sleeping");
    // Another user's send to this device id wakes nothing.
    const stranger = newUser();
    await call("POST", `/device/${deviceId}/nudge`, userBearer(stranger), { chatId });
    await new Promise((r) => setTimeout(r, 200));
    expect((await session(u, chatId))?.state).toBe("sleeping");

    // The owner's send does; the runner's first token after resume completes it.
    await call("POST", `/device/${deviceId}/nudge`, userBearer(u), { chatId });
    await waitForSession(u, chatId, "starting");
    await expect
      .poll(async () => (await boatState()).calls.some((c) => c.path.endsWith("/resume") && c.sandboxId === sandboxId))
      .toBe(true);
    expect((await boatState()).calls.find((c) => c.path.endsWith("/resume") && c.sandboxId === sandboxId)!.body).toEqual({ ttlSeconds: 7200 });
    await runnerToken(u, deviceId, key);
    await waitForSession(u, chatId, "ready");
  });

  it("caps awake sessions: evicts the least recently active idle one, refuses when all are busy", async () => {
    const u = newUser(); // CLOUD_MAX_AWAKE is 2 in the test config
    const project = await enableCloud(u);
    const a = await readySession(u, "chat-1", project);
    const b = await readySession(u, "chat-2", project);
    const beat = async (s: { deviceId: string; key: Awaited<ReturnType<typeof runnerKey>> }, activeRuns: number) =>
      call("POST", "/runner/heartbeat", `Bearer ${await runnerToken(u, s.deviceId, s.key)}`, { activeRuns, clients: 0 });
    // Both busy: a third session can't start.
    await beat(a, 1);
    await beat(b, 1);
    const refused = await createSession(u, "chat-3", project.spaceId);
    expect(refused.status).toBe(409);
    expect(await refused.json()).toMatchObject({ error: "too_many_awake" });
    expect(await session(u, "chat-3")).toBeUndefined();
    // chat-1 goes idle (and is the least recently active): it makes room.
    await beat(a, 0);
    await withSession(u, "chat-1", (s) => {
      s.lastActiveAt = Date.now() - 60 * 60_000;
    });
    expect((await createSession(u, "chat-3", project.spaceId)).status).toBe(200);
    await waitForSession(u, "chat-1", "sleeping");
    expect((await session(u, "chat-2"))?.state).toBe("ready");
    expect((await status(u)).awakeSessions).toBe(2);
  });

  it("surfaces a Boat key that cannot resume as a configuration error, and retries the exact action", async () => {
    const u = newUser();
    const { chatId, deviceId, sandboxId } = await readySession(u);
    expect((await call("POST", `/cloud/org1/sessions/${chatId}/sleep`, userBearer(u), {})).status).toBe(200);
    await waitForSession(u, chatId, "sleeping");

    await boatControl("__fail", { sandboxId, op: "resume", status: 403, code: "api_key_action_forbidden", action: "sandbox.resume" });
    await call("POST", `/device/${deviceId}/nudge`, userBearer(u), { chatId });
    const failed = (await waitForSession(u, chatId, "error"))!;
    expect(failed.error).toBe("Cloud isn't configured to wake sandboxes (the Boat API key lacks sandbox.resume).");
    expect(failed).toMatchObject({ failedAction: "wake" });

    const retried = (await (await call("POST", `/cloud/org1/sessions/${chatId}/wake`, userBearer(u), {})).json()) as SessionView;
    expect(retried.state).toBe("starting");
    expect(retried.failedAction).toBeUndefined();
  });
});

describe("delete", () => {
  it("deletes one session's machine exactly (stop, final usage, delete), leaving the others", async () => {
    const u = newUser();
    const project = await enableCloud(u);
    const keep = await readySession(u, "chat-keep", project);
    const { chatId, deviceId, key, sandboxId } = await readySession(u, "chat-del", project);
    await using deletes = await introspectWorkflow(env.CLOUD_DELETE);
    await deletes.modifyAll(async (m) => {
      await m.disableSleeps();
    });
    await boatControl("__set", { sandboxId, runningReads: 1 }); // meter lag right after the stop

    const deleting = (await (await call("DELETE", `/cloud/org1/sessions/${chatId}`, userBearer(u))).json()) as SessionView;
    expect(deleting.state).toBe("deleting");
    expect((await requestToken(u, deviceId, key)).status).toBe(403);
    await waitForSession(u, chatId, "gone");

    const calls = (await boatState()).calls.filter((c) => c.sandboxId === sandboxId);
    const stopAt = calls.findIndex((c) => c.path.endsWith("/stop"));
    const deleteAt = calls.findIndex((c) => c.method === "DELETE");
    const usageAt = calls.map((c, i) => (c.path.endsWith("/usage") ? i : -1)).filter((i) => i >= 0);
    expect(stopAt).toBeGreaterThanOrEqual(0);
    expect(deleteAt).toBeGreaterThan(stopAt);
    expect(usageAt.filter((i) => i > stopAt && i < deleteAt).length).toBeGreaterThanOrEqual(2);
    expect(usageAt.every((i) => i < deleteAt)).toBe(true);
    expect(calls[deleteAt]!.headers["x-ascii-confirm-delete"]).toBe(sandboxId);
    expect(calls.some((c) => c.path.endsWith("/stop") && (c.body as { force?: boolean } | undefined)?.force)).toBe(false);

    await runInDurableObject(accountStub(u), (_instance, state) => {
      const rows = listUsage(state.storage.sql).filter((r) => r.sandboxId === sandboxId);
      expect(rows.length).toBeGreaterThan(0);
      expect(rows.every((r) => r.closed && !r.running)).toBe(true);
      expect(listSandboxes(state.storage.sql).find((s) => s.sandboxId === sandboxId)?.chatId).toBe(chatId);
      const ledger = listLedger(state.storage.sql).filter((e) => e.chatId === chatId).map((e) => e.event);
      expect(ledger).toEqual(expect.arrayContaining(["create", "ready", "delete"]));
    });
    expect((await vaultCalls("revokeDevice")).some((c) => c.args[1] === deviceId)).toBe(true);
    expect((await session(u, "chat-keep"))?.state).toBe("ready");
    expect((await boatState()).sandboxes.find((s) => s.id === keep.sandboxId)?.deleted).toBe(false);

    const usage = (await (await call("GET", "/cloud/org1/usage", userBearer(u))).json()) as { sandboxes: { sandboxId: string; chatId?: string }[] };
    expect(usage.sandboxes.find((s) => s.sandboxId === sandboxId)?.chatId).toBe(chatId);
  });

  it("does not delete when the stop is refused for good; the error stays retryable", async () => {
    const u = newUser();
    const { chatId, sandboxId } = await readySession(u);
    await boatControl("__fail", { sandboxId, op: "stop", status: 403, code: "api_key_action_forbidden", action: "sandbox.stop" });
    await call("DELETE", `/cloud/org1/sessions/${chatId}`, userBearer(u));
    const failed = (await waitForSession(u, chatId, "error"))!;
    expect(failed.error).toBe("Cloud isn't configured to stop sandboxes (the Boat API key lacks sandbox.stop).");
    expect(failed).toMatchObject({ failedAction: "delete" });
    expect((await boatState()).calls.some((c) => c.sandboxId === sandboxId && c.method === "DELETE")).toBe(false);
    await call("DELETE", `/cloud/org1/sessions/${chatId}`, userBearer(u));
    await waitForSession(u, chatId, "gone");
    expect((await boatState()).sandboxes.find((s) => s.id === sandboxId)?.deleted).toBe(true);
  });

  it("turning Cloud off deletes every session and revokes the account", async () => {
    const u = newUser();
    const project = await enableCloud(u);
    const a = await readySession(u, "chat-x", project);
    const b = await readySession(u, "chat-y", project);
    await using deletes = await introspectWorkflow(env.CLOUD_DELETE);
    await deletes.modifyAll(async (m) => {
      await m.disableSleeps();
    });
    expect(((await (await call("DELETE", "/cloud/org1", userBearer(u))).json()) as { state: string }).state).toBe("deleting");
    await waitForState(u, "off");
    const sandboxes = (await boatState()).sandboxes;
    expect(sandboxes.find((s) => s.id === a.sandboxId)?.deleted).toBe(true);
    expect(sandboxes.find((s) => s.id === b.sandboxId)?.deleted).toBe(true);
    expect((await vaultCalls("revokeDevice")).some((c) => c.args[1] === a.accountDeviceId)).toBe(true);
    // Enabling again is a brand-new logical device.
    const again = (await (await call("POST", "/cloud/org1/enable", userBearer(u), {})).json()) as { deviceId: string };
    expect(again.deviceId).not.toBe(a.accountDeviceId);
  });
});

it("serves the fixture's preview route unchanged for user bearers", async () => {
  expect((await SELF.fetch("https://test/preview/foreign/ws?device=a", { headers: { authorization: "Bearer user@org", upgrade: "websocket" } })).status).toBe(403);
});
