import { env, runInDurableObject, runDurableObjectAlarm } from "cloudflare:test";
import { expect, it } from "vitest";
import { AUTH_USER_HEADER } from "../../src/env";
import type { Row } from "../../src/registry-core";

function room() { return env.REGISTRY_ROOMS.get(env.REGISTRY_ROOMS.idFromName(crypto.randomUUID())); }
const headers = { [AUTH_USER_HEADER]: "owner" };
async function rows(stub: DurableObjectStub) {
  const response = await stub.fetch("https://registry/rows", { headers });
  return response.json<{ rows: Row[]; presence: Record<string, number>; presenceNow: number }>();
}

it("retains old and future client-clock tombstones for server receipt age and keeps GC scheduled", async () => {
  const stub = room();
  const response = await stub.fetch("https://registry/push?device=test", {
    method: "POST", headers, body: JSON.stringify({ batch: crypto.randomUUID(), ops: [
      { kind: "devices", id: "past", op: "delete", hlc: "0000000000001-000000-test" },
      { kind: "devices", id: "future", op: "delete", hlc: "9000000000000-000000-test" },
    ] }),
  });
  expect(response.status).toBe(200);
  expect((await rows(stub)).rows.length).toBe(2);
  await runDurableObjectAlarm(stub);
  expect((await rows(stub)).rows.length).toBe(2);
  await runInDurableObject(stub, async (_instance, state) => {
    expect(await state.storage.getAlarm()).not.toBeNull();
    state.storage.sql.exec("UPDATE tombstone_receipts SET received_at = ?", Date.now() - 31 * 86400000);
  });
  await runDurableObjectAlarm(stub);
  expect((await rows(stub)).rows).toEqual([]);
  await runInDurableObject(stub, (_instance, state) => {
    expect([...state.storage.sql.exec("SELECT * FROM tombstone_receipts")]).toEqual([]);
    expect([...state.storage.sql.exec("SELECT value FROM meta WHERE key = 'gcFloor'")][0].value).toBe("1");
  });
});

it("presence is stamped at server receipt regardless of device epoch and ages out of snapshots", async () => {
  const stub = room();
  const response = await stub.fetch("https://registry/ws?device=wrong-clock", { headers: { ...headers, upgrade: "websocket" } });
  const ws = response.webSocket!;
  ws.accept();
  const messages: string[] = [];
  ws.addEventListener("message", e => { messages.push(String(e.data)); });
  ws.send(JSON.stringify({ t: "hello", device: "wrong-clock" }));
  await expect.poll(() => messages.length).toBeGreaterThan(0);
  ws.send(JSON.stringify({ t: "presence", at: Number.MAX_SAFE_INTEGER }));
  await expect.poll(async () => (await rows(stub)).presence["wrong-clock"]).toBeDefined();
  const snapshot = await rows(stub);
  expect(snapshot.presenceNow - snapshot.presence["wrong-clock"]).toBeGreaterThanOrEqual(0);
  expect(snapshot.presenceNow - snapshot.presence["wrong-clock"]).toBeLessThan(5000);
  await runInDurableObject(stub, (instance) => {
    const presence = (instance as unknown as { presence: Map<string, number> }).presence;
    presence.set("wrong-clock", Date.now() - 31000);
    presence.set("future", Date.now() + 31000);
  });
  expect((await rows(stub)).presence).toEqual({});
  ws.close();
});

it("backfills legacy tombstones conservatively and restarts a missing GC alarm", async () => {
  const stub = room();
  await stub.fetch("https://registry/push?device=test", {
    method: "POST", headers, body: JSON.stringify({ batch: crypto.randomUUID(), ops: [
      { kind: "devices", id: "legacy", op: "delete", hlc: "0000000000001-000000-test" },
    ] }),
  });
  const before = Date.now();
  await runInDurableObject(stub, async (_instance, state) => {
    state.storage.sql.exec("DELETE FROM tombstone_receipts");
    await state.storage.deleteAlarm();
    // Exercise the actual constructor migration against an existing SQLite row.
    const { RegistryRoom } = await import("../../src/registry-room");
    new RegistryRoom(state, env as unknown as import("../../src/env").Env);
    const receipt = [...state.storage.sql.exec("SELECT received_at FROM tombstone_receipts")][0];
    expect(receipt.received_at).toBeGreaterThanOrEqual(before);
  });
  await expect.poll(() => runInDurableObject(stub, (_instance, state) => state.storage.getAlarm())).not.toBeNull();
  expect((await rows(stub)).rows[0].deleted).toBe(true);
});
