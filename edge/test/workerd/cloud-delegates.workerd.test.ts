/**
 * Delegation from a Cloud machine: the machine of a main chat starts Cloud
 * sessions for child chats (each on a machine of its own), sees and manages
 * those children, and nothing else — no other main chat, no grandchildren.
 */
import { describe, expect, it } from "vitest";
import { encodeHlc, type Op } from "../../src/registry-core";
import {
  call,
  enrollCodeFor,
  enroll,
  newUser,
  readySession,
  runnerKey,
  runnerToken,
  session,
  userBearer,
  waitForSession,
  type TestUser
} from "./cloud-helpers";

/** A child of `parent` on a machine of its own, enrolled (ready), with its
 * runner's bearer. */
const readyChild = async (u: TestUser, runner: string, parent: string) => {
  const chatId = `child-${crypto.randomUUID().slice(0, 8)}`;
  const started = await call("POST", `/cloud/${u.orgId}/sessions`, runner, {
    chatId,
    parentChatId: parent,
    config: { harness: "codex" }
  });
  expect(started.status, await started.clone().text()).toBe(200);
  const { deviceId } = (await started.json()) as { deviceId: string };
  const key = await runnerKey();
  expect((await enroll(u, deviceId, await enrollCodeFor(deviceId), key.publicKey)).status).toBe(200);
  const bearer = `Bearer ${await runnerToken(u, deviceId, key)}`;
  await waitForSession(u, chatId, "ready");
  return { chatId, deviceId, bearer };
};

const rand = () => crypto.randomUUID().slice(0, 8);
let clock = 0;
const upsert = (kind: string, id: string, set: Op["set"], device: string): Op => ({
  kind,
  id,
  op: "upsert",
  set,
  hlc: encodeHlc(Date.now(), clock++, device)
});

type WireRow = { kind: string; id: string; deleted: boolean; fields: Record<string, unknown> };
const rows = async (bearer: string) =>
  ((await (await call("GET", "/registry/org1/rows", bearer)).json()) as { rows: WireRow[] }).rows;
const find = (list: WireRow[], kind: string, id: string) => list.find((r) => r.kind === kind && r.id === id && !r.deleted);

/** A main Cloud chat whose machine is up, with its chat row (as the laptop
 * that created it would write), plus a second, unrelated main chat. */
const world = async () => {
  const u: TestUser = { ...newUser("dg"), orgId: "org1" };
  const user = userBearer(u);
  const main = await readySession(u);
  const other = await readySession(u, undefined, { accountDeviceId: main.accountDeviceId, spaceId: main.spaceId });
  const push = await call("POST", "/registry/org1/push?device=laptop", user, {
    batch: `b-${rand()}`,
    ops: [
      upsert("chats", main.chatId, { deviceId: main.deviceId, spaceId: main.spaceId, title: "main" }, "laptop"),
      upsert("chats", other.chatId, { deviceId: other.deviceId, spaceId: other.spaceId, title: "other main" }, "laptop")
    ]
  });
  expect(push.status, await push.clone().text()).toBe(200);
  // The test deployment lets 2 machines run at once: the other main chat
  // sleeps, leaving room for a delegate.
  const slept = await call("POST", `/cloud/${u.orgId}/sessions/${other.chatId}/sleep`, user, {});
  expect(slept.status, await slept.clone().text()).toBe(200);
  await waitForSession(u, other.chatId, "sleeping");
  const runner = `Bearer ${await runnerToken(u, main.deviceId, main.key)}`;
  return { u, user, main, other, runner };
};

const startChild = (u: TestUser, runner: string, chatId: string, parentChatId: string, body: Record<string, unknown> = {}) =>
  call("POST", `/cloud/${u.orgId}/sessions`, runner, { chatId, parentChatId, config: { harness: "codex" }, ...body });

describe("Cloud delegates", () => {
  it("a delegate never takes an existing or deleted chat's id", async () => {
    const { u, user, main, other, runner } = await world();
    const laptopChat = `laptop-${rand()}`;
    const gone = `gone-${rand()}`;
    const push = await call("POST", "/registry/org1/push?device=laptop", user, {
      batch: `b-${rand()}`,
      ops: [
        upsert("chats", laptopChat, { deviceId: "laptop", spaceId: main.spaceId, title: "on the laptop" }, "laptop"),
        upsert("chats", gone, { deviceId: "laptop", spaceId: main.spaceId, title: "deleted" }, "laptop")
      ]
    });
    expect(push.status, await push.clone().text()).toBe(200);
    const del = await call("POST", "/registry/org1/push?device=laptop", user, {
      batch: `b-${rand()}`,
      ops: [{ kind: "chats", id: gone, op: "delete", hlc: encodeHlc(Date.now(), clock++, "laptop") }]
    });
    expect(del.status, await del.clone().text()).toBe(200);
    for (const taken of [laptopChat, gone, other.chatId]) {
      expect((await startChild(u, runner, taken, main.chatId)).status, taken).toBe(409);
    }
    // Nothing moved: the laptop's chat is still the laptop's, the deleted one stays deleted.
    const after = await rows(user);
    expect(find(after, "chats", laptopChat)?.fields.deviceId).toBe("laptop");
    expect(find(after, "chats", gone)).toBeUndefined();
  });

  it("a main chat's machine starts a child on a machine of its own, with its chat row", async () => {
    const { u, user, main, runner } = await world();
    const child = `child-${rand()}`;
    const started = await startChild(u, runner, child, main.chatId);
    expect(started.status, await started.clone().text()).toBe(200);
    const { deviceId } = (await started.json()) as { deviceId: string };
    expect(deviceId).not.toBe(main.deviceId);

    // The edge wrote the child's row: on its new machine, under its parent,
    // in the parent's project, with the asked-for agent.
    const row = find(await rows(user), "chats", child)!;
    expect(row.fields).toMatchObject({
      deviceId,
      parentChatId: main.chatId,
      spaceId: main.spaceId,
      roomGen: 2,
      archived: false,
      config: { harness: "codex" }
    });
    // Again: the same session, not a second machine.
    const again = await startChild(u, runner, child, main.chatId);
    expect(((await again.json()) as { deviceId: string }).deviceId).toBe(deviceId);
  });

  it("sees and reaches its children, never another main chat", async () => {
    const { u, main, other, runner } = await world();
    const child = `child-${rand()}`;
    const { deviceId } = (await (await startChild(u, runner, child, main.chatId)).json()) as { deviceId: string };

    const seen = await rows(runner);
    expect(find(seen, "chats", child)).toBeDefined();
    expect(find(seen, "chats", main.chatId)).toBeDefined();
    expect(find(seen, "chats", other.chatId)).toBeUndefined();

    // Chat rooms: its child's, not the other main chat's.
    expect((await call("GET", `/chat2/${child}/stats`, runner)).status).toBe(200);
    expect((await call("GET", `/chat2/${other.chatId}/stats`, runner)).status).toBe(403);
    // Device rooms: a client link and the nudge to its child's machine only.
    const link = await call("GET", `/device/${deviceId}/ws?role=client`, runner, undefined, { upgrade: "websocket" });
    expect(link.status).toBe(101);
    link.webSocket!.accept();
    link.webSocket!.close();
    expect((await call("GET", `/device/${deviceId}/ws?role=host`, runner, undefined, { upgrade: "websocket" })).status).toBe(403);
    expect((await call("GET", `/device/${other.deviceId}/ws?role=client`, runner, undefined, { upgrade: "websocket" })).status).toBe(403);
    expect((await call("POST", `/device/${other.deviceId}/nudge`, runner, { chatId: other.chatId })).status).toBe(403);
  });

  it("manages its children's machines, and only theirs", async () => {
    const { u, main, other, runner } = await world();
    const child = `child-${rand()}`;
    expect((await startChild(u, runner, child, main.chatId)).status).toBe(200);
    expect((await call("POST", `/cloud/${u.orgId}/sessions/${child}/sleep`, runner, {})).status).toBe(200);
    expect((await call("POST", `/cloud/${u.orgId}/sessions/${other.chatId}/sleep`, runner, {})).status).toBe(403);
    expect((await call("DELETE", `/cloud/${u.orgId}/sessions/${other.chatId}`, runner)).status).toBe(403);
    expect((await call("DELETE", `/cloud/${u.orgId}/sessions/${main.chatId}`, runner)).status).toBe(403);
    expect((await call("DELETE", `/cloud/${u.orgId}/sessions/${child}`, runner)).status).toBe(200);
    // Account-level Cloud stays the user's.
    expect((await call("GET", `/cloud/${u.orgId}/sessions`, runner)).status).toBe(403);
    expect((await call("DELETE", `/cloud/${u.orgId}`, runner)).status).toBe(403);
  });

  it("starts children only for the chat it hosts, and children start none", async () => {
    const { u, main, other, runner } = await world();
    // For another main chat, or as if it were the user (no parent).
    expect((await startChild(u, runner, `x-${rand()}`, other.chatId)).status).toBe(403);
    const asUser = await call("POST", `/cloud/${u.orgId}/sessions`, runner, {
      chatId: `x-${rand()}`,
      spaceId: main.spaceId,
      repo: { fullName: "someone/else", cloneUrl: "https://github.com/someone/else.git", defaultBranch: "main" }
    });
    expect(asUser.status).toBe(403); // no parentChatId: a machine never acts as the user
    // Only Cloud's agents.
    expect((await startChild(u, runner, `x-${rand()}`, main.chatId, { config: { harness: "cursor" } })).status).toBe(400);

    // A child's own machine can't start grandchildren.
    const child = `child-${rand()}`;
    const { deviceId } = (await (await startChild(u, runner, child, main.chatId)).json()) as { deviceId: string };
    const key = await runnerKey();
    expect((await enroll(u, deviceId, await enrollCodeFor(deviceId), key.publicKey)).status).toBe(200);
    const childRunner = `Bearer ${await runnerToken(u, deviceId, key)}`;
    const grandchild = await startChild(u, childRunner, `g-${rand()}`, child);
    expect(grandchild.status).toBe(403);
    expect(await grandchild.json()).toMatchObject({ error: "too_deep" });
  });

  it("wakes its sleeping child when it sends to it, and nobody else's", async () => {
    const { u, main, other, runner } = await world();
    const child = await readyChild(u, runner, main.chatId);
    expect((await call("POST", `/cloud/${u.orgId}/sessions/${child.chatId}/sleep`, runner, {})).status).toBe(200);
    await waitForSession(u, child.chatId, "sleeping");
    // Not a session it started: refused at the door, and nothing wakes.
    expect((await call("POST", `/device/${other.deviceId}/nudge`, runner, { chatId: other.chatId })).status).toBe(403);
    // Its own child: the send wakes it, as a user's send would.
    expect((await call("POST", `/device/${child.deviceId}/nudge`, runner, { chatId: child.chatId })).status).toBe(200);
    await waitForSession(u, child.chatId, "starting");
    expect((await session(u, other.chatId))?.state).toBe("sleeping");
  });

  it("wakes its sleeping parent once it stops working: finished, or waiting on an answer", async () => {
    const { u, user, main, runner } = await world();
    const child = await readyChild(u, runner, main.chatId);
    const state = async () => (await session(u, main.chatId))?.state;
    const beat = (body: Record<string, number>) =>
      call("POST", "/runner/heartbeat", child.bearer, { activeRuns: 0, clients: 0, ...body });
    expect((await beat({ activeRuns: 1 })).status).toBe(200);
    expect((await call("POST", `/cloud/${u.orgId}/sessions/${main.chatId}/sleep`, user, {})).status).toBe(200);
    await waitForSession(u, main.chatId, "sleeping");
    // Still working: the parent sleeps on.
    expect((await beat({ activeRuns: 1 })).status).toBe(200);
    await new Promise((r) => setTimeout(r, 200));
    expect(await state()).toBe("sleeping");
    // Waiting on an answer counts as stopped: the parent has to hear of it.
    expect((await beat({ activeRuns: 1, waitingRuns: 1 })).status).toBe(200);
    await waitForSession(u, main.chatId, "starting");
  });
});
