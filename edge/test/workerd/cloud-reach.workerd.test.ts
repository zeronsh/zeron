/**
 * A Cloud session's reach (registry-runner.ts): `devices` (the default) works
 * with the person's other devices as any of their computers does; `isolated`
 * keeps the machine to itself and its delegates. Only the person changes it,
 * and isolating closes the machine out of everything it reached
 * (runner-revocation.ts).
 */
import { describe, expect, it } from "vitest";
import { encodeHlc, type Op } from "../../src/registry-core";
import {
  call,
  enableCloud,
  enroll,
  enrollCodeFor,
  readySession,
  runnerKey,
  runnerToken,
  session,
  status,
  userBearer,
  waitForSession,
  newUser,
  REPO,
  type TestUser
} from "./cloud-helpers";

const rand = () => crypto.randomUUID().slice(0, 8);
let clock = 0;
const upsert = (kind: string, id: string, set: Op["set"], device: string): Op => ({
  kind,
  id,
  op: "upsert",
  set,
  hlc: encodeHlc(Date.now(), clock++, device)
});

type WireRow = {
  kind: string;
  id: string;
  deleted: boolean;
  fields: Record<string, unknown>;
};
const rows = async (bearer: string) =>
  (
    (await (await call("GET", "/registry/org1/rows", bearer)).json()) as {
      rows: WireRow[];
    }
  ).rows;
const find = (list: WireRow[], kind: string, id: string) => list.find((r) => r.kind === kind && r.id === id && !r.deleted);
const ws = { upgrade: "websocket" };

const push = async (bearer: string, ops: Op[], device: string) => {
  const reply = await call("POST", `/registry/org1/push?device=${device}`, bearer, { batch: `b-${rand()}`, ops });
  expect(reply.status, await reply.clone().text()).toBe(200);
  return (await reply.json()) as {
    applied: number;
    rejected?: { kind: string; id: string; error: string }[];
  };
};

/** A ready Cloud chat with its runner's bearer, and a laptop (online, with a
 * chat in a project) beside it. */
const world = async () => {
  const u: TestUser = newUser("rc");
  const user = userBearer(u);
  const main = await readySession(u);
  const laptop = `laptop-${rand()}`;
  const laptopChat = `lc-${rand()}`;
  const space = `sp-${rand()}`;
  await push(
    user,
    [
      upsert("devices", laptop, { name: "MacBook" }, laptop),
      upsert("spaces", space, { deviceId: laptop, name: "laptop project" }, laptop),
      upsert("chats", laptopChat, { deviceId: laptop, title: "laptop plans", spaceId: space }, laptop),
      upsert("sessions", laptopChat, { deviceId: laptop, status: "idle" }, laptop),
      upsert("chats", main.chatId, { deviceId: main.deviceId, spaceId: main.spaceId, title: "main" }, laptop)
    ],
    laptop
  );
  const host = await call("GET", `/device/${laptop}/ws?role=host`, user, undefined, ws);
  expect(host.status).toBe(101);
  host.webSocket!.accept();
  const runner = `Bearer ${await runnerToken(u, main.deviceId, main.key)}`;
  return {
    u,
    user,
    main,
    laptop,
    laptopChat,
    space,
    runner,
    host: host.webSocket!
  };
};

const setReach = (u: TestUser, bearer: string, chatId: string, reach: string) =>
  call("POST", `/cloud/${u.orgId}/sessions/${chatId}/reach`, bearer, { reach });

/** Open a socket and record how it closes. */
const open = async (path: string, bearer: string) => {
  const reply = await call("GET", path, bearer, undefined, ws);
  expect(reply.status, path).toBe(101);
  const socket = reply.webSocket!;
  const closed: number[] = [];
  socket.addEventListener("close", (e) => closed.push(e.code));
  socket.accept();
  return { socket, closed };
};

describe("reach defaults", () => {
  it("a session reaches the person's devices unless the person picks otherwise", async () => {
    const u = newUser("rd");
    const user = userBearer(u);
    expect((await status(u)) as unknown).toMatchObject({ reach: "devices" });
    const { spaceId } = await enableCloud(u);
    const first = await call("POST", `/cloud/${u.orgId}/sessions`, user, {
      chatId: `c-${rand()}`,
      spaceId,
      repo: REPO
    });
    expect(await first.json()).toMatchObject({ reach: "devices" });

    // The account's default, then a pick in the composer over it (another
    // account: the test deployment runs two machines at once).
    expect(
      (
        await call("PUT", `/cloud/${u.orgId}/reach`, user, {
          reach: "isolated"
        })
      ).status
    ).toBe(200);
    expect((await status(u)) as unknown).toMatchObject({ reach: "isolated" });
    const second = await call("POST", `/cloud/${u.orgId}/sessions`, user, {
      chatId: `c-${rand()}`,
      spaceId,
      repo: REPO
    });
    expect(await second.json()).toMatchObject({ reach: "isolated" });
    const v = newUser("rd");
    expect(
      (
        await call("PUT", `/cloud/${v.orgId}/reach`, userBearer(v), {
          reach: "isolated"
        })
      ).status
    ).toBe(200);
    const picked = await call("POST", `/cloud/${v.orgId}/sessions`, userBearer(v), {
      chatId: `c-${rand()}`,
      spaceId: (await enableCloud(v)).spaceId,
      repo: REPO,
      reach: "devices"
    });
    expect(await picked.json()).toMatchObject({ reach: "devices" });
    expect(
      (
        await call("PUT", `/cloud/${u.orgId}/reach`, user, {
          reach: "everything"
        })
      ).status
    ).toBe(400);
  });
});

describe("only the person sets a reach", () => {
  it("a machine changes neither its own, another session's, nor the default", async () => {
    const { u, main, runner } = await world();
    for (const reach of ["devices", "isolated"]) {
      expect((await setReach(u, runner, main.chatId, reach)).status).toBe(403);
      expect((await call("PUT", `/cloud/${u.orgId}/reach`, runner, { reach })).status).toBe(403);
    }
    expect((await session(u, main.chatId)) as unknown).toMatchObject({
      reach: "devices"
    });
  });

  it("a delegate reaches what its parent does, whatever it asks", async () => {
    const { u, user, main, runner } = await world();
    expect((await setReach(u, user, main.chatId, "isolated")).status).toBe(200);
    const child = `child-${rand()}`;
    const started = await call("POST", `/cloud/${u.orgId}/sessions`, runner, {
      chatId: child,
      parentChatId: main.chatId,
      config: { harness: "codex" },
      reach: "devices"
    });
    expect(started.status, await started.clone().text()).toBe(200);
    expect(await started.json()).toMatchObject({ reach: "isolated" });
    // Its delegate's machine stays its to reach.
    const { deviceId } = (await session(u, child))!;
    const link = await call("GET", `/device/${deviceId}/ws?role=client`, runner, undefined, ws);
    expect(link.status).not.toBe(403);
  });
});

describe("isolated", () => {
  it("sees and touches only its own rows, its projects and its delegates", async () => {
    const { u, user, main, laptop, laptopChat, space, runner } = await world();
    expect(find(await rows(runner), "chats", laptopChat)).toBeDefined();
    expect((await setReach(u, user, main.chatId, "isolated")).status).toBe(200);
    expect((await session(u, main.chatId)) as unknown).toMatchObject({
      reach: "isolated"
    });

    const seen = await rows(runner);
    expect(find(seen, "devices", laptop)).toBeDefined();
    expect(find(seen, "chats", main.chatId)).toBeDefined();
    expect(find(seen, "chats", laptopChat)).toBeUndefined();
    expect(find(seen, "sessions", laptopChat)).toBeUndefined();
    expect(find(seen, "spaces", space)).toBeUndefined();

    const refused = await push(
      runner,
      [
        upsert("chats", laptopChat, { title: "renamed from Cloud" }, main.deviceId),
        upsert("chats", `new-${rand()}`, { deviceId: laptop, title: "started from Cloud" }, main.deviceId)
      ],
      main.deviceId
    );
    expect(refused.applied).toBe(0);
    expect(find(await rows(user), "chats", laptopChat)?.fields.title).toBe("laptop plans");
  });

  it("reaches no other chat or device", async () => {
    const { u, user, main, laptop, laptopChat, runner } = await world();
    expect((await setReach(u, user, main.chatId, "isolated")).status).toBe(200);
    expect((await call("GET", `/chat2/${laptopChat}/stats`, runner)).status).toBe(403);
    expect((await call("GET", `/chat2/${laptopChat}--sub--toolu_01/stats`, runner)).status).toBe(403);
    expect((await call("PUT", `/blob/${laptopChat}/p1`, runner, "out")).status).toBe(403);
    expect((await call("GET", `/device/${laptop}/ws?role=client`, runner, undefined, ws)).status).toBe(403);
    expect(
      (
        await call("POST", `/device/${laptop}/nudge`, runner, {
          chatId: laptopChat
        })
      ).status
    ).toBe(403);
    // Its own chat stays its.
    expect((await call("GET", `/chat2/${main.chatId}/stats`, runner)).status).toBe(200);
    expect((await call("PUT", `/blob/${main.chatId}/p1`, runner, "out")).status).toBe(200);
  });

  it("wakes no other Cloud chat", async () => {
    const { u, user, main, runner } = await world();
    const other = await readySession(u, undefined, {
      accountDeviceId: main.accountDeviceId,
      spaceId: main.spaceId
    });
    await push(user, [upsert("chats", other.chatId, { deviceId: other.deviceId, title: "other" }, "laptop")], "laptop");
    expect((await call("POST", `/cloud/${u.orgId}/sessions/${other.chatId}/sleep`, user, {})).status).toBe(200);
    await waitForSession(u, other.chatId, "sleeping");
    expect((await setReach(u, user, main.chatId, "isolated")).status).toBe(200);
    expect(
      (
        await call("POST", `/device/${other.deviceId}/nudge`, runner, {
          chatId: other.chatId
        })
      ).status
    ).toBe(403);
    await new Promise((r) => setTimeout(r, 200));
    expect((await session(u, other.chatId))?.state).toBe("sleeping");
  });

  it("starts isolated when created so, before its machine is up", async () => {
    const u = newUser("ri");
    const { spaceId } = await enableCloud(u);
    const chatId = `c-${rand()}`;
    const created = await call("POST", `/cloud/${u.orgId}/sessions`, userBearer(u), { chatId, spaceId, repo: REPO, reach: "isolated" });
    expect(created.status).toBe(200);
    const { deviceId } = (await created.json()) as { deviceId: string };
    const key = await runnerKey();
    expect((await enroll(u, deviceId, await enrollCodeFor(deviceId), key.publicKey)).status).toBe(200);
    const runner = `Bearer ${await runnerToken(u, deviceId, key)}`;
    const laptopChat = `lc-${rand()}`;
    await push(userBearer(u), [upsert("chats", laptopChat, { deviceId: "laptop", title: "x" }, "laptop")], "laptop");
    expect(find(await rows(runner), "chats", laptopChat)).toBeUndefined();
    expect((await call("GET", `/chat2/${laptopChat}/stats`, runner)).status).toBe(403);
  });
});

describe("switching", () => {
  it("isolating closes the machine out of every chat and device it reached, at once", async () => {
    const { u, user, main, laptop, laptopChat, runner, host } = await world();
    const chat = await open(`/chat2/${laptopChat}/ws?device=${main.deviceId}`, runner);
    const device = await open(`/device/${laptop}/ws?role=client`, runner);
    const registry = await open("/registry/org1/ws", runner);
    const own = await open(`/chat2/${main.chatId}/ws?device=${main.deviceId}`, runner);

    expect((await setReach(u, user, main.chatId, "isolated")).status).toBe(200);
    await expect.poll(() => chat.closed).toEqual([4403]);
    await expect.poll(() => device.closed).toEqual([4403]);
    await expect.poll(() => registry.closed).toEqual([1012]);
    await new Promise((r) => setTimeout(r, 100));
    expect(own.closed).toEqual([]); // its own chat holds in every reach
    // And it can't come back.
    expect((await call("GET", `/chat2/${laptopChat}/ws?device=${main.deviceId}`, runner, undefined, ws)).status).toBe(403);
    expect((await call("GET", `/device/${laptop}/ws?role=client`, runner, undefined, ws)).status).toBe(403);
    // Again: nothing left to close.
    expect((await setReach(u, user, main.chatId, "isolated")).status).toBe(200);

    // Back to the person's devices: the next sync is whole again.
    expect((await setReach(u, user, main.chatId, "devices")).status).toBe(200);
    expect((await call("GET", `/chat2/${laptopChat}/stats`, runner)).status).toBe(200);
    const pulled = (await (await call("GET", `/registry/org1/rows?since=999999`, runner)).json()) as { full: boolean; rows: WireRow[] };
    expect(pulled.full).toBe(true);
    expect(find(pulled.rows, "chats", laptopChat)).toBeDefined();
    own.socket.close();
    host.close();
  });

  it("a dial admitted before the switch, arriving after it, is refused", async () => {
    const { u, user, main, laptop, runner, host } = await world();
    // Admitted (devices) and recorded, then isolated before it connects:
    // the room took the drop first.
    const first = await open(`/device/${laptop}/ws?role=client`, runner);
    first.socket.close();
    expect((await setReach(u, user, main.chatId, "isolated")).status).toBe(200);
    expect((await call("GET", `/device/${laptop}/ws?role=client`, runner, undefined, ws)).status).toBe(403);
    host.close();
  });
});
