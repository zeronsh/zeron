import { env, runInDurableObject, runDurableObjectAlarm } from "cloudflare:test";
import { expect, it } from "vitest";
import { ensureNudges, enqueueNudge, pendingNudges, acknowledgeNudge, NUDGE_CAP } from "../../src/device-nudges";
import { decodeDeviceFrame, encodeDeviceFrame } from "../../src/device-room";

it("migrates legacy receipts, keeps more than 256 wakes, and fences stale acknowledgments", async () => {
  const room = env.TEST_LOG.get(env.TEST_LOG.idFromName(crypto.randomUUID()));
  await runInDurableObject(room, (_instance, state) => {
    const sql = state.storage.sql;
    sql.exec("CREATE TABLE pending_nudges(chat_id TEXT PRIMARY KEY,queued_at INTEGER NOT NULL)");
    sql.exec("INSERT INTO pending_nudges VALUES ('old',1)");
    ensureNudges(sql);
    const old = pendingNudges(sql)[0];
    expect(old.token).not.toBe("");
    enqueueNudge(sql, "old");
    acknowledgeNudge(sql, "old", old.token);
    expect(pendingNudges(sql).find(r => r.chat_id === "old")).toBeDefined();
    for (let i=0; i<NUDGE_CAP-1; i++) expect(enqueueNudge(sql, `chat-${i}`)).toBe(true);
    expect(enqueueNudge(sql, "overflow")).toBe(false);
    expect([...sql.exec("SELECT count(*) AS n FROM pending_nudges")][0].n).toBe(NUDGE_CAP);
    expect(pendingNudges(sql)[0].chat_id).toBe("*");
  });
});

it("replays unacknowledged frames and retires only the matching token", async () => {
  const room = env.DEVICE_ROOMS.get(env.DEVICE_ROOMS.idFromName(crypto.randomUUID()));
  const headers = { "x-zeron-auth-user": "owner", upgrade: "websocket" };
  const response = await room.fetch("https://test/ws?role=host&nudgeAck=1", { headers });
  expect(response.status).toBe(101);
  const ws = response.webSocket!; ws.accept(); ws.binaryType = "arraybuffer";
  const messages: Array<{ chatId: string; token: string }> = [];
  ws.addEventListener("message", e => { const frame=decodeDeviceFrame(new Uint8Array(e.data as ArrayBuffer)); messages.push(JSON.parse(new TextDecoder().decode(frame.payload))); });
  const post = () => room.fetch("https://test/nudge", { method: "POST", headers: { "x-zeron-auth-user": "owner" }, body: JSON.stringify({chatId:"chat"}) });
  const posted = await post();
  expect(posted.status, await posted.text()).toBe(200);
  await expect.poll(() => messages.length).toBe(1);
  const first = messages[0];
  await runDurableObjectAlarm(room);
  await expect.poll(() => messages.length).toBe(2);
  expect(messages[1]).toEqual(first);
  await post();
  await expect.poll(() => messages.length).toBe(3);
  const newer = messages[2];
  ws.send(encodeDeviceFrame({s:"chat", k:"nudgeAck"}, new TextEncoder().encode(JSON.stringify(first))));
  await runInDurableObject(room, (_i, state) => { expect(pendingNudges(state.storage.sql)[0].token).toBe(newer.token); });
  ws.send(encodeDeviceFrame({s:"chat", k:"nudgeAck"}, new TextEncoder().encode(JSON.stringify(newer))));
  await expect.poll(() => runInDurableObject(room, (_i, state) => pendingNudges(state.storage.sql).length)).toBe(0);
  ws.close();
});

it("drains 300 queued wakes through a bounded acknowledgment window", async () => {
  const room = env.DEVICE_ROOMS.get(env.DEVICE_ROOMS.idFromName(crypto.randomUUID()));
  const response = await room.fetch("https://test/ws?role=host&nudgeAck=1", { headers: { "x-zeron-auth-user": "owner", upgrade: "websocket" } });
  const ws = response.webSocket!; ws.accept(); ws.binaryType = "arraybuffer";
  const seen = new Set<string>();
  ws.addEventListener("message", e => {
    const frame = decodeDeviceFrame(new Uint8Array(e.data as ArrayBuffer));
    const receipt = JSON.parse(new TextDecoder().decode(frame.payload));
    seen.add(receipt.chatId);
    ws.send(encodeDeviceFrame({s:receipt.chatId,k:"nudgeAck"},frame.payload));
  });
  await runInDurableObject(room, async (_i, state) => {
    for (let i=0;i<300;i++) enqueueNudge(state.storage.sql, `chat-${i}`);
    await state.storage.setAlarm(Date.now()+5000);
  });
  await runDurableObjectAlarm(room);
  await expect.poll(() => seen.size, {timeout:5000}).toBe(300);
  await expect.poll(() => runInDurableObject(room, (_i, state) => pendingNudges(state.storage.sql).length)).toBe(0);
  ws.close();
});

it("keeps legacy hosts usable without acknowledgment support", async () => {
  const room = env.DEVICE_ROOMS.get(env.DEVICE_ROOMS.idFromName(crypto.randomUUID()));
  const response = await room.fetch("https://test/ws?role=host", { headers: { "x-zeron-auth-user": "owner", upgrade: "websocket" } });
  const ws = response.webSocket!; ws.accept(); ws.binaryType = "arraybuffer";
  const messages: string[] = [];
  ws.addEventListener("message", e => {
    const frame = decodeDeviceFrame(new Uint8Array(e.data as ArrayBuffer));
    messages.push(JSON.parse(new TextDecoder().decode(frame.payload)).chatId);
  });
  await room.fetch("https://test/nudge", {method:"POST",headers:{"x-zeron-auth-user":"owner"},body:JSON.stringify({chatId:"legacy"})});
  await expect.poll(() => messages).toEqual(["legacy"]);
  await runInDurableObject(room, async (_i, state) => {
    expect(pendingNudges(state.storage.sql)).toEqual([]);
    await state.storage.deleteAlarm();
  });
  ws.close();
});
