import { SELF } from "cloudflare:test";
import { describe, expect, it } from "vitest";
import { UPDATE_CLOSE_CODE, UPDATE_HEADER } from "../../src/compat";

/** Version compatibility (docs/compat.md) through the production routes, on
 * real Durable Objects. */

const bearer = (user: string) => ({ authorization: `Bearer ${user}@org` });

type Socket = { ws: WebSocket; heard: Record<string, unknown>[] };

/** A registry socket; `proto` undefined dials like a build from before
 * protocols (no `proto=`). */
async function registry(user: string, device: string, proto?: number): Promise<Socket> {
  const query = proto === undefined ? "" : `&proto=${proto}`;
  const response = await SELF.fetch(`https://edge/registry/org/ws?device=${device}${query}`, {
    headers: { ...bearer(user), upgrade: "websocket" }
  });
  expect(response.status).toBe(101);
  const ws = response.webSocket!;
  ws.accept();
  const heard: Record<string, unknown>[] = [];
  ws.addEventListener("message", (event) => heard.push(JSON.parse(event.data as string)));
  ws.send(JSON.stringify({ t: "hello", device, cursor: null }));
  await expect.poll(() => heard.some((f) => f.t === "state")).toBe(true);
  return { ws, heard };
}

const beats = (socket: Socket) => socket.heard.filter((f) => f.t === "presence");
const state = (socket: Socket) => socket.heard.find((f) => f.t === "state") as Record<string, unknown>;

describe("presence by protocol", () => {
  it("hides protocol-aware devices from builds that announce none", async () => {
    const user = `presence-${crypto.randomUUID()}`;
    const viewer = await registry(user, "viewer", 1);
    const old = await registry(user, "old");
    const updated = await registry(user, "updated", 1);

    updated.ws.send(JSON.stringify({ t: "presence", at: 111 }));
    await expect.poll(() => beats(viewer)).toContainEqual({ t: "presence", device: "updated", at: 111, proto: 1 });
    old.ws.send(JSON.stringify({ t: "presence", at: 222 }));
    // The old build's beat reaches everyone, without a protocol for it.
    await expect.poll(() => beats(viewer)).toContainEqual({ t: "presence", device: "old", at: 222, proto: 0 });
    await expect.poll(() => beats(updated)).toContainEqual({ t: "presence", device: "old", at: 222, proto: 0 });
    expect(beats(old), "the updated device reads offline to the old build").toEqual([]);

    // Late joiners: the hello answer filters the same way.
    const lateOld = await registry(user, "late-old");
    expect(state(lateOld).presence).toEqual({ old: 222 });
    expect(state(lateOld)).not.toHaveProperty("presenceProto");
    const lateNew = await registry(user, "late-new", 1);
    expect(state(lateNew).presence).toEqual({ updated: 111, old: 222 });
    expect(state(lateNew).presenceProto).toEqual({ updated: 1, old: 0 });

    // The HTTPS pull too: the protocol rides the header there.
    const pull = async (headers: Record<string, string>) =>
      (await (await SELF.fetch("https://edge/registry/org/rows?device=poller", { headers: { ...bearer(user), ...headers } })).json()) as Record<string, unknown>;
    expect((await pull({})).presence).toEqual({ old: 222 });
    expect((await pull({ "x-zeron-proto": "1" })).presence).toEqual({ updated: 111, old: 222 });
    for (const socket of [viewer, old, updated, lateOld, lateNew]) socket.ws.close();
  });
});

describe("a moved chat2 room", () => {
  it("refuses every caller once its chat3 host marks it, and keeps answering the host", async () => {
    const user = `moved-${crypto.randomUUID()}`;
    const chat = crypto.randomUUID();
    // An old build already sitting in the room.
    const response = await SELF.fetch(`https://edge/chat2/${chat}/ws?device=old`, { headers: { ...bearer(user), upgrade: "websocket" } });
    expect(response.status).toBe(101);
    const sitting = response.webSocket!;
    sitting.accept();
    const closed = new Promise<number>((resolve) => sitting.addEventListener("close", (e) => resolve(e.code), { once: true }));

    const mark = () => SELF.fetch(`https://edge/chat2/${chat}/moved`, { method: "POST", headers: bearer(user) });
    expect((await mark()).status).toBe(200);
    expect(await closed).toBe(UPDATE_CLOSE_CODE);
    expect((await mark()).status, "idempotent for the host").toBe(200);

    const refused = async (response: Response) => {
      expect(response.status).toBe(426);
      expect(response.headers.get(UPDATE_HEADER)).toBe("room");
    };
    await refused(await SELF.fetch(`https://edge/chat2/${chat}/ws?device=old`, { headers: { ...bearer(user), upgrade: "websocket" } }));
    await refused(await SELF.fetch(`https://edge/chat2/${chat}/checkpoint`, { headers: bearer(user) }));
    await refused(await SELF.fetch(`https://edge/chat2/${chat}/rows?batchId=b1&device=old`, { method: "POST", headers: bearer(user), body: new Uint8Array([1]) }));
    // Someone else's chat id is not theirs to mark.
    expect((await SELF.fetch(`https://edge/chat2/${chat}/moved`, { method: "POST", headers: bearer("intruder") })).status).toBe(403);
    // The chat3 room of the same chat is untouched.
    expect((await SELF.fetch(`https://edge/chat3/${chat}/stats`, { headers: bearer(user) })).status).toBe(200);
  });
});

describe("a protocol-aware device to older builds", () => {
  const dial = (user: string, device: string, role: "host" | "client", proto?: number) =>
    SELF.fetch(`https://edge/device/${device}/ws?role=${role}${proto === undefined ? "" : `&proto=${proto}`}`, {
      headers: { ...bearer(user), upgrade: "websocket" }
    });
  const status = async (user: string, device: string, headers: Record<string, string> = {}) =>
    ((await (await SELF.fetch(`https://edge/device/${device}/status`, { headers: { ...bearer(user), ...headers } })).json()) as { hostConnected: boolean }).hostConnected;

  it("reads offline to their liveness probe and refuses their peer links", async () => {
    const user = `device-${crypto.randomUUID()}`;
    const device = `dev-${crypto.randomUUID()}`;
    // The host's old build claims its room and an old peer link joins it;
    // then the host updates and reconnects.
    const before = await dial(user, device, "host");
    before.webSocket!.accept();
    const waiting = await dial(user, device, "client");
    expect(waiting.status).toBe(101);
    waiting.webSocket!.accept();
    const kicked = new Promise<number>((resolve) => waiting.webSocket!.addEventListener("close", (e) => resolve(e.code), { once: true }));
    const host = await dial(user, device, "host", 1);
    expect(host.status).toBe(101);
    host.webSocket!.accept();
    expect(await kicked).toBe(UPDATE_CLOSE_CODE);

    expect(await status(user, device)).toBe(false);
    expect(await status(user, device, { "x-zeron-proto": "1" })).toBe(true);
    const old = await dial(user, device, "client");
    expect(old.status).toBe(426);
    const updated = await dial(user, device, "client", 1);
    expect(updated.status).toBe(101);
    updated.webSocket!.accept();
    updated.webSocket!.close();
    host.webSocket!.close();
  });

  it("leaves an old host reachable by everyone", async () => {
    const user = `device-${crypto.randomUUID()}`;
    const device = `dev-${crypto.randomUUID()}`;
    const host = await dial(user, device, "host");
    host.webSocket!.accept();
    expect(await status(user, device)).toBe(true);
    const old = await dial(user, device, "client");
    expect(old.status).toBe(101);
    old.webSocket!.accept();
    old.webSocket!.close();
    host.webSocket!.close();
  });
});
