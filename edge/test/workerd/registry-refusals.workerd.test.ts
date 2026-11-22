import { env } from "cloudflare:test";
import { describe, expect, it } from "vitest";
import { AUTH_USER_HEADER } from "../../src/env";
import { MAX_OP_BYTES, type Op } from "../../src/registry-core";

/** A refused push names its batch and says it is for good, so a client can
 * stop queueing it. Before, the refusal named nothing: the batch stayed
 * queued, was re-sent on every reconnect, and over HTTPS stopped every
 * write queued behind it. */

const H = { [AUTH_USER_HEADER]: "user" };
const room = (name: string) => env.REGISTRY_ROOMS.get(env.REGISTRY_ROOMS.idFromName(`reg1/org/${name}`));

let tick = 0;
const hlc = () => `${String(Date.now()).padStart(13, "0")}-${String(++tick % 1_000_000).padStart(6, "0")}-mac`;

/** A session row whose background task carries an 11 KB heredoc twice, as
 * a command and as its description: the shape that was refused. */
const oversizedSession = (chat: string): Op => {
  const heredoc = `cat > tests/settings.rs <<'EOF'\n${"// settings test line\n".repeat(530)}EOF`;
  return {
    kind: "sessions",
    id: chat,
    op: "upsert",
    hlc: hlc(),
    set: {
      chatId: chat,
      deviceId: "mac",
      status: "idle",
      updatedAt: Date.now(),
      backgroundTasks: [{ id: "b1", command: heredoc, description: heredoc, startedAt: "2026-11-14T03:14:13Z" }]
    }
  };
};
const smallChat = (id: string): Op => ({ kind: "chats", id, op: "upsert", hlc: hlc(), set: { id, deviceId: "mac", title: "Fix login" } });

const pushHttp = (name: string, batch: string, ops: Op[]) =>
  room(name).fetch("https://registry/push?device=mac", {
    method: "POST",
    headers: H,
    body: JSON.stringify({ batch, ops })
  });

describe("a refused push names its batch", () => {
  it("over HTTPS: an oversized op is refused for good, and the next batch still applies", async () => {
    const op = oversizedSession("c1");
    expect(JSON.stringify(op).length).toBeGreaterThan(MAX_OP_BYTES);
    const refused = await pushHttp("http-refusal", "b-big", [op]);
    expect(refused.status).toBe(400);
    expect(await refused.json()).toEqual({
      error: "invalid_op",
      message: "sessions/c1: op too large",
      batch: "b-big",
      permanent: true
    });
    const next = await pushHttp("http-refusal", "b-next", [smallChat("c2")]);
    expect(next.status).toBe(200);
    expect(await next.json()).toMatchObject({ batch: "b-next", applied: 1 });
  });

  it("over HTTPS: a malformed push names no batch and is not marked for good", async () => {
    const refused = await room("http-malformed").fetch("https://registry/push?device=mac", {
      method: "POST",
      headers: H,
      body: JSON.stringify({ ops: [smallChat("c1")] })
    });
    expect(refused.status).toBe(400);
    expect(await refused.json()).toEqual({ error: "bad_push", message: "malformed push" });
  });

  it("over the socket: the error frame names the batch, and the next push is acked", async () => {
    const response = await room("ws-refusal").fetch("https://registry/ws?device=mac", { headers: { ...H, upgrade: "websocket" } });
    const socket = response.webSocket!;
    socket.accept();
    const frames: Record<string, unknown>[] = [];
    socket.addEventListener("message", (event) => frames.push(JSON.parse(event.data as string)));
    socket.send(JSON.stringify({ t: "hello", cursor: null, device: "mac" }));
    await expect.poll(() => frames.some((f) => f.t === "state")).toBe(true);

    socket.send(JSON.stringify({ t: "push", batch: "b-big", ops: [oversizedSession("c1")] }));
    socket.send(JSON.stringify({ t: "push", batch: "b-next", ops: [smallChat("c2")] }));
    await expect.poll(() => frames.some((f) => f.t === "ack" && f.batch === "b-next")).toBe(true);
    expect(frames.find((f) => f.t === "error")).toEqual({
      t: "error",
      code: "invalid_op",
      message: "sessions/c1: op too large",
      batch: "b-big",
      permanent: true
    });
    expect(frames.some((f) => f.t === "ack" && f.batch === "b-big")).toBe(false);
    socket.close();
  });
});
