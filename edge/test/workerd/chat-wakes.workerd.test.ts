import { env, SELF, runInDurableObject, runDurableObjectAlarm } from "cloudflare:test";
import { expect, it } from "vitest";
import { decodeFrame, encodeFrame, FRAME } from "../../src/chat-frames";
import { decodeDeviceFrame } from "../../src/device-room";
import { enqueueNudge, NUDGE_CAP, pendingNudges } from "../../src/device-nudges";
import { finishChatWake, pendingChatWake, queueChatWake, retryChatWake } from "../../src/chat-wakes";
import { appendRow, getMeta, logStats, rowsAfter, setMeta } from "../../src/chat-log";

const owner = "sender-owner";
const auth = { authorization: `Bearer ${owner}@org` };
const chatRoom = (chat: string) => env.CHAT_ROOMS.get(env.CHAT_ROOMS.idFromName(`chat2/${chat}`));
const deviceRoom = (host: string) => env.DEVICE_ROOMS.get(env.DEVICE_ROOMS.idFromName(`d2/${host}`));
const post = (chat: string, host: string, batch = "prompt") => SELF.fetch(
  `https://edge/chat2/${chat}/rows?device=phone&hostDevice=${host}&batchId=${batch}&chatId=spoofed-chat`,
  { method: "POST", headers: auth, body: new Uint8Array([1, 2, 3]) }
);
async function hostSocket(host: string, user = owner) {
  const response = await deviceRoom(host).fetch("https://device/ws?role=host&nudgeAck=1", {
    headers: { "x-zeron-auth-user": user, upgrade: "websocket" }
  });
  expect(response.status).toBe(101);
  const socket = response.webSocket!;
  socket.accept(); socket.binaryType = "arraybuffer";
  const wakes: Array<{ chatId: string; token: string }> = [];
  socket.addEventListener("message", event => {
    const frame = decodeDeviceFrame(new Uint8Array(event.data as ArrayBuffer));
    wakes.push(JSON.parse(new TextDecoder().decode(frame.payload)));
  });
  return { socket, wakes };
}
async function retryNow(chat: string) {
  const room = chatRoom(chat);
  await runInDurableObject(room, (_instance, state) => { state.storage.sql.exec("UPDATE chat_wake SET next_at=0"); });
  await runDurableObjectAlarm(room);
}
const waitForRetry = (chat: string) => expect.poll(() => runInDurableObject(chatRoom(chat), (_i, state) => pendingChatWake(state.storage.sql)?.attempts)).toBeGreaterThan(0);

it("delivers an HTTPS-accepted prompt with the phone offline and no relaunch or separate nudge", async () => {
  const chat = crypto.randomUUID(), host = crypto.randomUUID();
  const response = await post(chat, host);
  expect(response.status, await response.text()).toBe(200);
  await waitForRetry(chat);
  await runInDurableObject(chatRoom(chat), async (_i, state) => {
    const wake = pendingChatWake(state.storage.sql)!;
    expect(wake.chat_id).toBe(chat); // route defeats spoofed query
    expect(wake.host_device).toBe(host);
    expect(logStats(state.storage.sql).rowCount).toBe(1);
    expect(await state.storage.getAlarm()).not.toBeNull();
    expect(await state.storage.getAlarm()).toBeLessThanOrEqual(Date.now() + 2000);
    expect(Number([...state.storage.sql.exec("SELECT value FROM meta WHERE key='backupDue'")][0].value)).toBeGreaterThan(Date.now() + 60_000);
  });
  // No more phone requests. Only the server alarm transfers the obligation.
  const desktop = await hostSocket(host);
  await retryNow(chat);
  await expect.poll(() => desktop.wakes.map(w => w.chatId)).toContain(chat);
  await runInDurableObject(chatRoom(chat), (_i, state) => { expect(pendingChatWake(state.storage.sql)).toBeUndefined(); });
  const pull = await SELF.fetch(`https://edge/chat2/${chat}/rows?device=${host}`, { headers: auth });
  expect(pull.status).toBe(200);
  const bytes = new Uint8Array(await pull.arrayBuffer());
  const frames = [];
  for (let offset = 0; offset < bytes.length;) {
    const length = new DataView(bytes.buffer).getUint32(offset, true); offset += 4;
    frames.push(decodeFrame(bytes.subarray(offset, offset + length))); offset += length;
  }
  expect(frames.find(f => f?.type === FRAME.row)?.payload).toEqual(new Uint8Array([1, 2, 3]));
  desktop.socket.close();
});

it("keeps a websocket-accepted prompt's wake after the phone socket closes", async () => {
  const chat = crypto.randomUUID(), host = crypto.randomUUID();
  const response = await SELF.fetch(`https://edge/chat2/${chat}/ws?device=phone&hostDevice=${host}`, { headers: { ...auth, upgrade: "websocket" } });
  expect(response.status).toBe(101);
  const phone = response.webSocket!; phone.accept(); phone.binaryType = "arraybuffer";
  const kinds: number[] = [];
  phone.addEventListener("message", event => { kinds.push(decodeFrame(new Uint8Array(event.data as ArrayBuffer))!.type); });
  phone.send(encodeFrame(FRAME.hello, { device: "phone", cursor: 0 }));
  await expect.poll(() => kinds).toContain(FRAME.state);
  phone.send(encodeFrame(FRAME.push, { batchId: "phone-prompt" }, new Uint8Array([4, 5])));
  await expect.poll(() => kinds).toContain(FRAME.ack);
  phone.close();
  await waitForRetry(chat);
  const desktop = await hostSocket(host);
  await retryNow(chat);
  await expect.poll(() => desktop.wakes.map(w => w.chatId)).toContain(chat);
  await runInDurableObject(chatRoom(chat), (_i, state) => {
    expect([...rowsAfter(state.storage.sql, 0)][0].bytes).toEqual(new Uint8Array([4, 5]));
    expect(pendingChatWake(state.storage.sql)).toBeUndefined();
  });
  desktop.socket.close();
});

it("retains accepted bytes through device queue saturation and hands off with both devices offline", async () => {
  const chat = crypto.randomUUID(), host = crypto.randomUUID();
  const desktop = await hostSocket(host); desktop.socket.close();
  await runInDurableObject(deviceRoom(host), (_i, state) => {
    for (let i = 0; i < NUDGE_CAP; i++) enqueueNudge(state.storage.sql, `queued-${i}`);
  });
  const response = await post(chat, host); expect(response.status, await response.text()).toBe(200);
  await waitForRetry(chat);
  await runInDurableObject(deviceRoom(host), (_i, state) => { state.storage.sql.exec("DELETE FROM pending_nudges WHERE chat_id='queued-0'"); });
  await retryNow(chat);
  await runInDurableObject(chatRoom(chat), (_i, state) => { expect(pendingChatWake(state.storage.sql)).toBeUndefined(); });
  await runInDurableObject(deviceRoom(host), (_i, state) => {
    expect([...state.storage.sql.exec("SELECT chat_id FROM pending_nudges WHERE chat_id=?", chat)][0].chat_id).toBe(chat);
  });
});

it("coalesces wakes, fences stale forwards, and rolls back the receipt and alarm together", async () => {
  await runInDurableObject(chatRoom(crypto.randomUUID()), async (_i, state) => {
    const sql = state.storage.sql;
    queueChatWake(sql, "chat", "host", owner); const older = pendingChatWake(sql)!;
    queueChatWake(sql, "chat", "host", owner); const newer = pendingChatWake(sql)!;
    finishChatWake(sql, older.token); retryChatWake(sql, older);
    expect(pendingChatWake(sql)).toEqual(newer);
    await expect(state.storage.transaction(async () => {
      appendRow(sql, "phone", "uncommitted", new Uint8Array([1]), Date.now());
      queueChatWake(sql, "rollback", "host", owner);
      await state.storage.setAlarm(Date.now() + 1000);
      throw new Error("crash before commit");
    })).rejects.toThrow("crash before commit");
    expect(pendingChatWake(sql)).toEqual(newer);
    expect(logStats(sql).rowCount).toBe(0);
    expect(await state.storage.getAlarm()).toBeNull();
  });
});

it("keeps nightly backups scheduled after wake delivery without backing up on every wake alarm", async () => {
  const chat = crypto.randomUUID(), host = crypto.randomUUID();
  const desktop = await hostSocket(host);
  const response = await post(chat, host); expect(response.status).toBe(200); await response.text();
  await expect.poll(() => runInDurableObject(chatRoom(chat), (_i, state) => pendingChatWake(state.storage.sql))).toBeUndefined();
  await runDurableObjectAlarm(chatRoom(chat));
  await runInDurableObject(chatRoom(chat), async (_i, state) => {
    expect(getMeta(state.storage.sql, "backupSeq")).toBeUndefined();
    expect(await state.storage.getAlarm()).toBe(Number(getMeta(state.storage.sql, "backupDue")));
    setMeta(state.storage.sql, "backupDue", "0");
  });
  await runDurableObjectAlarm(chatRoom(chat));
  await runInDurableObject(chatRoom(chat), async (_i, state) => {
    expect(getMeta(state.storage.sql, "backupSeq")).toBe("1");
    expect(getMeta(state.storage.sql, "backupDirty")).toBe("0");
    expect(await state.storage.getAlarm()).toBeNull();
  });
  desktop.socket.close();
});

it("deduplicates replays, avoids host-output wake loops, and preserves legacy pushes", async () => {
  const chat = crypto.randomUUID(), host = crypto.randomUUID();
  for (let i = 0; i < 2; i++) { const response = await post(chat, host); expect(response.status).toBe(200); await response.text(); }
  await runInDurableObject(chatRoom(chat), (_i, state) => { expect(logStats(state.storage.sql).rowCount).toBe(1); });
  for (const own of [true, false]) {
    const ownChat = crypto.randomUUID();
    const response = await SELF.fetch(`https://edge/chat2/${ownChat}/rows?device=${host}&batchId=output${own ? `&hostDevice=${host}` : ""}`, {
      method: "POST", headers: auth, body: new Uint8Array([1])
    });
    expect(response.status).toBe(200); await response.text();
    await runInDurableObject(chatRoom(ownChat), (_i, state) => { expect(pendingChatWake(state.storage.sql)).toBeUndefined(); });
  }
});

it("accepts rows with an unroutable wake, rejects unauthorized pushes, and drops a wake another owner's device refuses", async () => {
  const chat = crypto.randomUUID();
  // A malformed wake hint never refuses the user's message: the row lands,
  // only host discovery is skipped.
  const bad = await SELF.fetch(`https://edge/chat2/${chat}/rows?device=phone&hostDevice=invalid/host&batchId=bad`, {
    method: "POST", headers: auth, body: new Uint8Array([1])
  });
  expect(bad.status, await bad.text()).toBe(200);
  await runInDurableObject(chatRoom(chat), (_i, state) => {
    expect(logStats(state.storage.sql).rowCount).toBe(1); expect(pendingChatWake(state.storage.sql)).toBeUndefined();
  });
  const forbidden = await SELF.fetch(`https://edge/chat2/${chat}/rows?batchId=bad&hostDevice=host`, {
    method: "POST", headers: { authorization: "Bearer stranger@org" }
  });
  expect(forbidden.status).toBe(403); await forbidden.text();
  // Another owner's device room refuses the forward (403): no retry can
  // succeed, so the receipt is dropped rather than retried forever.
  const host = crypto.randomUUID(); const other = await hostSocket(host, "stranger");
  const accepted = await post(chat, host, "foreign"); expect(accepted.status).toBe(200); await accepted.text();
  await retryNow(chat);
  await expect.poll(() => runInDurableObject(chatRoom(chat), (_i, state) => pendingChatWake(state.storage.sql))).toBeUndefined();
  await runInDurableObject(deviceRoom(host), (_i, state) => { expect(pendingNudges(state.storage.sql)).toEqual([]); });
  other.socket.close();
});

it("moves the backup deadline a day on when rows land during the upload", async () => {
  const chat = crypto.randomUUID();
  const room = chatRoom(chat);
  const response = await post(chat, crypto.randomUUID(), "first"); expect(response.status).toBe(200); await response.text();
  await runInDurableObject(room, async (instance, state) => {
    const sql = state.storage.sql;
    sql.exec("DELETE FROM chat_wake");
    setMeta(sql, "backupDue", "0");
    await state.storage.setAlarm(Date.now());
    // The chat keeps streaming while the R2 put is in flight.
    const target = instance as unknown as { env: { BLOBS: { put: (...args: unknown[]) => Promise<unknown> } } };
    const real = target.env.BLOBS;
    target.env = { ...target.env, BLOBS: { ...real, put: async (...args: unknown[]) => {
      appendRow(sql, "host", `during-${crypto.randomUUID()}`, new Uint8Array([9]), Date.now());
      return real.put.apply(real, args);
    } } };
  });
  // Fire it if the runtime has not already.
  await runDurableObjectAlarm(room);
  await expect.poll(() => runInDurableObject(room, (_i, state) => getMeta(state.storage.sql, "backupSeq"))).toBe("1");
  await runInDurableObject(room, async (_instance, state) => {
    const sql = state.storage.sql;
    expect(getMeta(sql, "backupSeq")).toBe("1");
    // The new row waits for the NEXT nightly backup, not an immediate re-run.
    expect(getMeta(sql, "backupDirty")).toBe("1");
    const due = Number(getMeta(sql, "backupDue"));
    expect(due).toBeGreaterThan(Date.now() + 23 * 60 * 60 * 1000);
    expect(await state.storage.getAlarm()).toBe(due);
  });
});
