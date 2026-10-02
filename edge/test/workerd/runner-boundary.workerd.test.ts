/**
 * The Cloud runner trust boundary (docs/design/cloud-device.md, "Trust
 * boundary"): a runner token — possibly stolen by code running in the
 * sandbox — acts as its device, never as the whole user.
 */
import { describe, expect, it } from "vitest";
import { encodeHlc, type Op } from "../../src/registry-core";
import { call, newUser, userBearer, type TestUser } from "./cloud-helpers";

const rand = () => crypto.randomUUID().slice(0, 8);
let clock = 0;
const hlc = (device: string) => encodeHlc(Date.now(), clock++, device);
const upsert = (kind: string, id: string, set: Op["set"], device: string): Op => ({ kind, id, op: "upsert", set, hlc: hlc(device) });

const push = async (bearer: string, ops: Op[], device = "laptop") => {
  const reply = await call("POST", `/registry/org1/push?device=${device}`, bearer, { batch: `b-${rand()}`, ops });
  expect(reply.status, await reply.clone().text()).toBe(200);
  return (await reply.json()) as { applied: number; rejected?: { kind: string; id: string; error: string }[] };
};

type WireRow = { kind: string; id: string; deleted: boolean; fields: Record<string, unknown> };
const rows = async (bearer: string) =>
  ((await (await call("GET", "/registry/org1/rows", bearer)).json()) as { rows: WireRow[] }).rows;
const find = (list: WireRow[], kind: string, id: string) => list.find((r) => r.kind === kind && r.id === id);

/** A user with a laptop-hosted chat/session/space and a Cloud device. */
const world = async () => {
  const u: TestUser = newUser("tb");
  const laptop = `laptop-${rand()}`;
  const cloud = `cloud-${rand()}`;
  const user = userBearer(u);
  const runner = `Bearer runner:${u.userId}@org1:${cloud}`;
  const laptopChat = `lc-${rand()}`;
  const space = `sp-${rand()}`;
  await push(user, [
    upsert("devices", laptop, { name: "MacBook" }, laptop),
    upsert("spaces", space, { deviceId: laptop, name: "secret space" }, laptop),
    upsert("chats", laptopChat, { deviceId: laptop, title: "secret plans", spaceId: space }, laptop),
    upsert("sessions", laptopChat, { deviceId: laptop, status: "running" }, laptop)
  ]);
  return { u, user, runner, laptop, cloud, laptopChat, space };
};

const ws = { upgrade: "websocket" };

describe("device rooms", () => {
  it("a runner cannot dial, nudge or read sidecars of another device, but can see its liveness", async () => {
    const { user, runner, laptop } = await world();
    const host = await call("GET", `/device/${laptop}/ws?role=host`, user, undefined, ws);
    expect(host.status).toBe(101);
    host.webSocket!.accept();

    expect((await call("GET", `/device/${laptop}/ws?role=client`, runner, undefined, ws)).status).toBe(403);
    expect((await call("GET", `/device/${laptop}/ws?role=host`, runner, undefined, ws)).status).toBe(403);
    expect((await call("POST", `/device/${laptop}/nudge`, runner, { chatId: "c1" })).status).toBe(403);
    expect((await call("GET", `/device/${laptop}/sidecar/repos`, runner)).status).toBe(403);
    const status = await call("GET", `/device/${laptop}/status`, runner);
    expect(status.status).toBe(200);
    expect(await status.json()).toMatchObject({ hostConnected: true });

    // The user's own devices are unaffected.
    const client = await call("GET", `/device/${laptop}/ws?role=client`, user, undefined, ws);
    expect(client.status).toBe(101);
    client.webSocket!.accept();
    expect((await call("POST", `/device/${laptop}/nudge`, user, { chatId: "c1" })).status).toBe(200);
    client.webSocket!.close();
    host.webSocket!.close();
  });

  it("a runner cannot use the legacy rooms or registry internals", async () => {
    const { user, runner } = await world();
    for (const [method, path] of [
      ["GET", "/session/c1/ws"],
      ["GET", "/tail/c1"],
      ["GET", "/snapshot/c1"],
      ["POST", "/append/c1"],
      ["GET", "/workspace/org1/tail"],
      ["GET", "/registry/org1/stats"]
    ]) {
      expect((await call(method!, path!, runner)).status, `${method} ${path}`).toBe(403);
    }
    expect((await call("GET", "/registry/org1/stats", user)).status).toBe(200);
  });
});

describe("chat-scoped data", () => {
  it("opens chat2 and blobs only for chats the runner's device hosts", async () => {
    const { user, runner, cloud, laptopChat } = await world();
    const cloudChat = `cc-${rand()}`;

    const denied = await call("GET", `/chat2/${laptopChat}/stats`, runner);
    expect(denied.status).toBe(403);
    expect(await denied.json()).toMatchObject({ error: "not_host" });
    // Not in the registry yet: refused — and not remembered as refused.
    expect((await call("GET", `/chat2/${cloudChat}/stats`, runner)).status).toBe(403);
    expect((await push(runner, [upsert("chats", cloudChat, { deviceId: cloud, title: "cloud work" }, cloud)], cloud)).applied).toBe(1);
    expect((await call("GET", `/chat2/${cloudChat}/stats`, runner)).status).toBe(200);

    expect((await call("PUT", `/blob/${laptopChat}/p1`, runner, "out")).status).toBe(403);
    expect((await call("PUT", `/blob/${cloudChat}/p1`, runner, "out")).status).toBe(200);

    // User bearers are untouched.
    expect((await call("GET", `/chat2/${laptopChat}/stats`, user)).status).toBe(200);
    expect((await call("PUT", `/blob/${laptopChat}/p1`, user, "out")).status).toBe(200);
  });

  it("refuses a runner's attempt to claim a laptop chat, so the chat stays closed to it", async () => {
    const { user, runner, cloud, laptop, laptopChat } = await world();
    const claim = await push(runner, [upsert("chats", laptopChat, { deviceId: cloud }, cloud)], cloud);
    expect(claim.applied).toBe(0);
    expect(claim.rejected).toEqual([{ kind: "chats", id: laptopChat, error: "hosted by another device" }]);
    expect(find(await rows(user), "chats", laptopChat)?.fields).toMatchObject({ deviceId: laptop, title: "secret plans" });
    expect((await call("GET", `/chat2/${laptopChat}/stats`, runner)).status).toBe(403);
  });
});

describe("registry writes", () => {
  it("skips the ops a runner doesn't own and applies the rest of the batch", async () => {
    const { user, runner, cloud, laptop, laptopChat, space } = await world();
    const cloudChat = `cc-${rand()}`;
    const result = await push(
      runner,
      [
        upsert("devices", cloud, { name: "Cloud", platform: "cloud" }, cloud),
        upsert("chats", cloudChat, { deviceId: cloud, title: "mine" }, cloud),
        upsert("devices", laptop, { name: "pwned" }, cloud),
        upsert("sessions", laptopChat, { status: "failed" }, cloud),
        upsert("spaces", space, { deviceId: cloud }, cloud),
        { kind: "chats", id: laptopChat, op: "delete", hlc: hlc(cloud) },
        upsert("preferences", "sidebar", { collapsed: true }, cloud)
      ],
      cloud
    );
    expect(result.applied).toBe(2);
    expect(result.rejected?.map((r) => `${r.kind}/${r.id}`)).toEqual([
      `devices/${laptop}`,
      `sessions/${laptopChat}`,
      `spaces/${space}`,
      `chats/${laptopChat}`,
      "preferences/sidebar"
    ]);
    const all = await rows(user);
    expect(find(all, "devices", laptop)?.fields.name).toBe("MacBook");
    expect(find(all, "sessions", laptopChat)?.fields.status).toBe("running");
    expect(find(all, "chats", laptopChat)?.deleted).toBe(false);
    expect(find(all, "chats", cloudChat)?.fields.deviceId).toBe(cloud);

    // The same writes from the user's own bearer are not restricted.
    expect((await push(user, [upsert("devices", laptop, { name: "Renamed" }, laptop)])).applied).toBe(1);
  });
});

describe("registry reads", () => {
  it("delivers only devices and the runner's own rows — over HTTP, hello and live broadcasts", async () => {
    const { user, runner, cloud, laptop, laptopChat, space } = await world();
    const cloudChat = `cc-${rand()}`;
    await push(runner, [upsert("devices", cloud, { name: "Cloud" }, cloud), upsert("chats", cloudChat, { deviceId: cloud, title: "mine" }, cloud)], cloud);

    const seen = await rows(runner);
    expect(find(seen, "devices", laptop)).toBeDefined();
    expect(find(seen, "devices", cloud)).toBeDefined();
    expect(find(seen, "chats", cloudChat)).toBeDefined();
    expect(find(seen, "chats", laptopChat)).toBeUndefined();
    expect(find(seen, "sessions", laptopChat)).toBeUndefined();
    expect(find(seen, "spaces", space)).toBeUndefined();
    expect(JSON.stringify(seen)).not.toContain("secret");
    const everything = await rows(user);
    expect(find(everything, "chats", laptopChat)).toBeDefined();

    // WebSocket: the hello state is filtered, and so is every broadcast.
    const socket = (await call("GET", "/registry/org1/ws", runner, undefined, ws)).webSocket!;
    socket.accept();
    const frames: Record<string, unknown>[] = [];
    socket.addEventListener("message", (event) => {
      frames.push(JSON.parse(event.data as string));
    });
    socket.send(JSON.stringify({ t: "hello", cursor: null, device: laptop }));
    await expect.poll(() => frames.some((f) => f.t === "state")).toBe(true);
    const state = frames.find((f) => f.t === "state") as { rows: WireRow[] };
    expect(find(state.rows, "chats", cloudChat)).toBeDefined();
    expect(find(state.rows, "chats", laptopChat)).toBeUndefined();

    await push(user, [upsert("chats", laptopChat, { title: "more secret plans" }, laptop)]);
    await push(user, [upsert("chats", cloudChat, { title: "renamed from the laptop" }, laptop)]);
    await expect.poll(() => frames.filter((f) => f.t === "rows").length).toBe(2);
    const broadcasts = frames.filter((f) => f.t === "rows") as { rows: WireRow[] }[];
    expect(broadcasts[0]!.rows).toEqual([]); // the cursor still advances
    expect(broadcasts[1]!.rows.map((r) => r.id)).toEqual([cloudChat]);
    expect(JSON.stringify(frames)).not.toContain("secret");

    // Its pushes over the socket get per-op errors plus a partial ack, and
    // its presence/attribution is its verified device, not the claimed one.
    socket.send(JSON.stringify({ t: "push", batch: "ws-1", ops: [upsert("chats", laptopChat, { title: "x" }, cloud)] }));
    await expect.poll(() => frames.some((f) => f.t === "ack" && f.batch === "ws-1")).toBe(true);
    expect(frames.find((f) => f.t === "error" && f.code === "not_owner")).toMatchObject({
      message: `chats/${laptopChat}: hosted by another device`
    });
    expect(frames.find((f) => f.t === "ack" && f.batch === "ws-1")).toMatchObject({ applied: 0, rejected: [expect.objectContaining({ id: laptopChat })] });
    socket.close();
  });
});
