/**
 * Attachment bytes awaiting a Cloud machine (src/attachment-blobs.ts) against a
 * real R2 binding: who may touch them, the size limit, the checksum.
 */
import { SELF, env } from "cloudflare:test";
import { describe, expect, it } from "vitest";
import { MAX_ATTACHMENT_BYTES, SHA256_HEADER, attachmentKey } from "../../src/attachment-blobs";
import { encodeHlc, type Op } from "../../src/registry-core";
import { call, newUser, userBearer, type TestUser } from "./cloud-helpers";

const rand = () => crypto.randomUUID().slice(0, 8);
let clock = 0;
const upsert = (kind: string, id: string, set: Op["set"], device: string): Op => ({
  kind,
  id,
  op: "upsert",
  set,
  hlc: encodeHlc(Date.now(), clock++, device)
});

const push = async (bearer: string, ops: Op[], device: string) => {
  const reply = await call("POST", `/registry/org1/push?device=${device}`, bearer, { batch: `b-${rand()}`, ops });
  expect(reply.status).toBe(200);
};

const sha256 = async (bytes: Uint8Array) =>
  [...new Uint8Array(await crypto.subtle.digest("SHA-256", bytes))].map((b) => b.toString(16).padStart(2, "0")).join("");

const put = (bearer: string | undefined, path: string, bytes: Uint8Array, headers: Record<string, string> = {}) =>
  SELF.fetch(`https://edge.test${path}`, {
    method: "PUT",
    headers: { ...(bearer ? { authorization: bearer } : {}), ...headers },
    body: bytes
  });

const PNG = new TextEncoder().encode("not really a png, but bytes");

/** A user whose laptop sends to a chat hosted on their Cloud machine. */
const world = async () => {
  const u: TestUser = newUser("att");
  const cloud = `cloud-${rand()}`;
  const other = `cloud-${rand()}`;
  const chat = `chat-${rand()}`;
  const user = userBearer(u);
  await push(user, [upsert("chats", chat, { deviceId: cloud, title: "cloud work" }, "laptop")], "laptop");
  return {
    u,
    user,
    host: `Bearer runner:${u.userId}@org1:${cloud}`,
    otherMachine: `Bearer runner:${u.userId}@org1:${other}`,
    cloud,
    other,
    chat,
    path: `/blob/${chat}/attachments/up-${rand()}`
  };
};

describe("attachment bytes", () => {
  it("a sender stores them, the chat's machine reads and then deletes them", async () => {
    const { user, host, path } = await world();
    const stored = await put(user, path, PNG, { [SHA256_HEADER]: await sha256(PNG) });
    expect(stored.status, await stored.clone().text()).toBe(200);

    const head = await call("HEAD", path, host);
    expect(head.status).toBe(200);
    await head.body?.cancel();

    const read = await call("GET", path, host);
    expect(read.status).toBe(200);
    expect(read.headers.get("content-type")).toBe("application/octet-stream");
    expect(read.headers.get("cache-control")).toBe("private, no-store");
    expect(read.headers.get(SHA256_HEADER)).toBe(await sha256(PNG));
    expect(new Uint8Array(await read.arrayBuffer())).toEqual(PNG);

    const deleted = await call("DELETE", path, host);
    expect(deleted.status).toBe(200);
    await deleted.body?.cancel();
    const gone = await call("GET", path, host);
    expect(gone.status).toBe(404);
    await gone.body?.cancel();
  });

  it("is stored under its own prefix, apart from tool outputs, for the lifecycle rule", async () => {
    const { u, user, chat } = await world();
    const stored = await put(user, `/blob/${chat}/attachments/keyed`, PNG);
    expect(stored.status).toBe(200);
    await stored.body?.cancel();
    const key = attachmentKey(u.userId, chat, "keyed");
    expect(key.startsWith("attachments/")).toBe(true);
    const object = await env.BLOBS.head(key);
    expect(object?.size).toBe(PNG.length);
  });

  it("an unauthenticated caller gets nothing", async () => {
    const { user, path } = await world();
    await (await put(user, path, PNG)).body?.cancel();
    for (const [method, send] of [
      ["GET", () => call("GET", path)],
      ["HEAD", () => call("HEAD", path)],
      ["DELETE", () => call("DELETE", path)],
      ["PUT", () => put(undefined, path, PNG)]
    ] as const) {
      const reply = await send();
      expect(reply.status, method).toBe(401);
      await reply.body?.cancel();
    }
  });

  it("another user's bearer cannot read, overwrite or delete them", async () => {
    const { user, path, u, chat } = await world();
    await (await put(user, path, PNG)).body?.cancel();
    const stranger = userBearer(newUser("stranger"));

    const read = await call("GET", path, stranger);
    expect(read.status).toBe(404);
    await read.body?.cancel();

    // Writes and deletes land in the stranger's own namespace, not this one.
    await (await put(stranger, path, new TextEncoder().encode("overwritten"))).body?.cancel();
    await (await call("DELETE", path, stranger)).body?.cancel();
    const uploadId = path.split("/").at(-1)!;
    const mine = await env.BLOBS.get(attachmentKey(u.userId, chat, uploadId));
    expect(new Uint8Array(await mine!.arrayBuffer())).toEqual(PNG);
  });

  it("a machine that doesn't host the chat is refused, whatever it asks", async () => {
    const { user, otherMachine, path } = await world();
    await (await put(user, path, PNG)).body?.cancel();
    for (const [method, send] of [
      ["GET", () => call("GET", path, otherMachine)],
      ["HEAD", () => call("HEAD", path, otherMachine)],
      ["DELETE", () => call("DELETE", path, otherMachine)],
      ["PUT", () => put(otherMachine, path, PNG)]
    ] as const) {
      const reply = await send();
      expect(reply.status, method).toBe(403);
      if (method !== "HEAD") expect(await reply.json(), method).toMatchObject({ error: "not_host" });
      else await reply.body?.cancel();
    }
    const still = await call("GET", path, user);
    expect(new Uint8Array(await still.arrayBuffer())).toEqual(PNG);
  });

  it("the machine that started a delegate may reach the delegate's attachments", async () => {
    const { user, host, cloud, other, otherMachine } = await world();
    const parent = `parent-${rand()}`;
    const child = `child-${rand()}`;
    await push(
      user,
      [
        upsert("chats", parent, { deviceId: cloud, title: "parent" }, "laptop"),
        upsert("chats", child, { deviceId: other, parentChatId: parent, title: "child" }, "laptop")
      ],
      "laptop"
    );
    const path = `/blob/${child}/attachments/up-${rand()}`;
    const stored = await put(host, path, PNG);
    expect(stored.status, await stored.clone().text()).toBe(200);
    expect((await call("GET", path, otherMachine)).status).toBe(200);
  });

  it("refuses anything over the engine's upload cap before storing it", async () => {
    const { user, path, u, chat } = await world();
    const over = await put(user, path, new Uint8Array(MAX_ATTACHMENT_BYTES + 1));
    expect(over.status).toBe(413);
    await over.body?.cancel();
    expect(await env.BLOBS.head(attachmentKey(u.userId, chat, path.split("/").at(-1)!))).toBeNull();
  });

  it("wants the length up front", async () => {
    const { user, path } = await world();
    const reply = await SELF.fetch(`https://edge.test${path}`, {
      method: "PUT",
      headers: { authorization: user },
      body: new ReadableStream({
        start(controller) {
          controller.enqueue(PNG);
          controller.close();
        }
      })
    });
    expect(reply.status).toBe(411);
    await reply.body?.cancel();
  });

  it("stores nothing when the bytes don't match their checksum", async () => {
    const { user, path, u, chat } = await world();
    const reply = await put(user, path, PNG, { [SHA256_HEADER]: "0".repeat(64) });
    expect(reply.status).toBe(503);
    await reply.body?.cancel();
    expect(await env.BLOBS.head(attachmentKey(u.userId, chat, path.split("/").at(-1)!))).toBeNull();

    const malformed = await put(user, path, PNG, { [SHA256_HEADER]: "nope" });
    expect(malformed.status).toBe(400);
    await malformed.body?.cancel();
  });

  it("leaves the tool-output sidecar route as it was", async () => {
    const { user, chat } = await world();
    const stored = await SELF.fetch(`https://edge.test/blob/${chat}/part1`, {
      method: "PUT",
      headers: { authorization: user, "content-type": "text/plain" },
      body: "output"
    });
    expect(stored.status).toBe(200);
    await stored.body?.cancel();
    const read = await call("GET", `/blob/${chat}/part1`, user);
    expect(await read.text()).toBe("output");
    // Malformed ids under the new route never reach R2.
    const bad = await call("GET", `/blob/${chat}/attachments/a.b`, user);
    expect(bad.status).toBe(404);
    await bad.body?.cancel();
  });
});
