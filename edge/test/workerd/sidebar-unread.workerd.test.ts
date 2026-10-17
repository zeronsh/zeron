import { env } from "cloudflare:test";
import { describe, expect, it } from "vitest";
import { AUTH_USER_HEADER } from "../../src/env";
import { type Op, type Row } from "../../src/registry-core";

const hlc = (tick: number) => `${String(tick).padStart(13, "0")}-000000-test`;
const chatOp = (set: NonNullable<Op["set"]>, tick: number, op: Op["op"] = "update"): Op => ({
  kind: "chats", id: "chat-unread", op, set, hlc: hlc(tick),
});
const room = () => env.REGISTRY_ROOMS.get(env.REGISTRY_ROOMS.idFromName(crypto.randomUUID()));

async function push(stub: DurableObjectStub, ops: Op[]) {
  const response = await stub.fetch("https://registry/push?device=test", {
    method: "POST",
    headers: { [AUTH_USER_HEADER]: "user" },
    body: JSON.stringify({ batch: crypto.randomUUID(), ops }),
  });
  expect(response.status).toBe(200);
  return response.json<{ batch: string; seq: number; applied: number }>();
}

async function chat(stub: DurableObjectStub) {
  const response = await stub.fetch("https://registry/rows", {
    headers: { [AUTH_USER_HEADER]: "user" },
  });
  const body = await response.json<{ rows: Row[] }>();
  return body.rows.find(row => row.kind === "chats" && row.id === "chat-unread");
}

describe("session unread intent on real SQLite", () => {
  it("retains the delete clock and rejects an obsolete seen marker", async () => {
    const stub = room();
    await push(stub, [chatOp({ lastMessageAt: 100, lastSeenAt: 100 }, 1, "upsert")]);

    expect((await push(stub, [chatOp({ lastSeenAt: null }, 3)])).applied).toBe(1);
    let row = await chat(stub);
    expect(row?.fields).toEqual({ lastMessageAt: 100 });
    expect(row?.clocks.lastSeenAt).toBe(hlc(3));

    expect((await push(stub, [chatOp({ lastSeenAt: 100 }, 2)])).applied).toBe(0);
    row = await chat(stub);
    expect(row?.fields.lastSeenAt).toBeUndefined();
    expect(row?.clocks.lastSeenAt).toBe(hlc(3));

    expect((await push(stub, [chatOp({ lastSeenAt: 100 }, 4)])).applied).toBe(1);
    row = await chat(stub);
    expect(row?.fields.lastSeenAt).toBe(100);
    expect(row?.clocks.lastSeenAt).toBe(hlc(4));
  });
});
