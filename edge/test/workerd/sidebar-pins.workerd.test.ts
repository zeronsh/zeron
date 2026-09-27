import { env } from "cloudflare:test";
import { describe, expect, it } from "vitest";
import { AUTH_USER_HEADER } from "../../src/env";
import { type Op, type Row } from "../../src/registry-core";

const op = (id: string, set: NonNullable<Op["set"]>, tick = 1): Op => ({
  kind: "sidebarPins", id, set, op: "upsert", hlc: `${String(tick).padStart(13, "0")}-000000-test`,
});
function room() { return env.REGISTRY_ROOMS.get(env.REGISTRY_ROOMS.idFromName(crypto.randomUUID())); }
async function push(stub: DurableObjectStub, ops: Op[]) {
  const response = await stub.fetch("https://registry/push?device=test", {
    method: "POST", headers: { [AUTH_USER_HEADER]: "user" },
    body: JSON.stringify({ batch: crypto.randomUUID(), ops }),
  });
  expect(response.status).toBe(200);
  return response.json<{ batch: string; seq: number; applied: number }>();
}
async function rows(stub: DurableObjectStub) {
  const response = await stub.fetch("https://registry/rows", { headers: { [AUTH_USER_HEADER]: "user" } });
  return (await response.json<{ rows: Row[] }>()).rows;
}

describe("per-pin storage on real SQLite", () => {
  it("moves one pin without rewriting another pin or the session", async () => {
    const stub = room();
    await push(stub, [op("a", { pinned: true, orderKey: "8" }), op("b", { pinned: true, orderKey: "c" }),
      { kind: "chats", id: "a", op: "upsert", set: { lastMessageAt: 123 }, hlc: "0000000000001-000000-test" }]);
    const before = await rows(stub);
    expect((await push(stub, [op("a", { orderKey: "e" }, 2)])).applied).toBe(1);
    const after = await rows(stub);
    for (const row of before.filter(row => row.kind !== "sidebarPins" || row.id !== "a")) {
      expect(after.find(next => next.kind === row.kind && next.id === row.id)).toEqual(row);
    }
    expect(after.find(row => row.kind === "sidebarPins" && row.id === "a")?.fields).toEqual({ pinned: true, orderKey: "e" });
  });

  it("retains unpin membership under a delayed move and duplicate delivery", async () => {
    const stub = room();
    const initial = op("a", { pinned: true, orderKey: "8" });
    await push(stub, [initial]);
    await push(stub, [op("a", { pinned: false }, 2)]);
    await push(stub, [op("a", { orderKey: "c" }, 3)]);
    expect((await push(stub, [initial])).applied).toBe(0);
    expect((await rows(stub))[0].fields).toEqual({ pinned: false, orderKey: "c" });
  });
});

describe("synced sidebar sections on real SQLite", () => {
  it("stores metadata and resolves placement without replacing session data", async () => {
    const stub = room();
    const section = (set: NonNullable<Op["set"]>, tick: number): Op =>
      ({ ...op("focus", set, tick), kind: "sidebarSections" });
    const placement = (location: string, tick: number): Op =>
      ({ ...op("session", { location }, tick), kind: "sidebarLocations" });
    await push(stub, [
      section({ name: "Focus", collapsed: false, deleted: false, createdAt: "1" }, 1),
      { ...op("session", { title: "Keep me" }), kind: "chats" },
      placement("focus", 1),
    ]);
    await push(stub, [section({ name: "Today" }, 3), section({ collapsed: true }, 2)]);
    await push(stub, [placement("pinned", 3), placement("focus", 2)]);
    let current = await rows(stub);
    expect(current.find(r => r.kind === "sidebarSections")?.fields).toMatchObject({ name: "Today", collapsed: true });
    expect(current.find(r => r.kind === "sidebarLocations")?.fields).toEqual({ location: "pinned" });
    await push(stub, [section({ deleted: true }, 4)]);
    current = await rows(stub);
    expect(current.find(r => r.kind === "sidebarSections")?.fields.deleted).toBe(true);
    expect(current.find(r => r.kind === "chats")?.fields).toEqual({ title: "Keep me" });
  });
});

describe("draft send reservations", () => {
  it("reserves one revision and replays the same send after an uncertain response", async () => {
    const stub = room();
    const claim = (revision: string) => stub.fetch("https://registry/draft-claim", {
      method: "POST", headers: { [AUTH_USER_HEADER]: "user" }, body: JSON.stringify({ id: "draft-a", revision }),
    });
    const responses = await Promise.all([claim("rev-a"), claim("rev-b")]);
    expect(responses.map(r => r.status).sort()).toEqual([200, 409]);
    const winner = responses[0].status === 200 ? "rev-a" : "rev-b";
    expect((await claim(winner)).status).toBe(200);
  });
  it("does not reserve a discarded draft", async () => {
    const stub = room();
    await push(stub, [{ ...op("draft-a", { closed: true }), kind: "promptDrafts" }]);
    const response = await stub.fetch("https://registry/draft-claim", { method: "POST", headers: { [AUTH_USER_HEADER]: "user" }, body: JSON.stringify({ id: "draft-a", revision: "rev" }) });
    expect(response.status).toBe(409);
  });
});
