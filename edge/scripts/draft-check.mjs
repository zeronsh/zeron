// Draft-room wire-level E2E against `wrangler dev --var AUTH_MODE:dev` or a
// dev-auth deployment. Speaks the chat2 binary frames (edge/src/chat-frames.ts)
// against /draft/:orgId/:chatId/* (edge/src/draft-room.ts, docs/draft-sync.md).
// Usage: node scripts/draft-check.mjs <baseUrl>
//        npm run draft-check -- <baseUrl>
import { randomBytes, randomUUID } from "node:crypto";

const base = process.argv[2];
if (!base) throw new Error("usage: node draft-check.mjs <baseUrl>");
const wsBase = base.replace(/^http/, "ws");
const ORG = "org1";
const alice = `alice@${ORG}`;
const bob = `bob@${ORG}`;
const chat = `draft-${randomUUID().slice(0, 13)}`;

// ── frame codec (mirror of chat-frames.ts) ──────────────────────────────────
const FRAME = { hello: 0x01, state: 0x02, rowsReq: 0x03, row: 0x04, rowsDone: 0x05, push: 0x06, ack: 0x07, presence: 0x08, probe: 0x09, probeOk: 0x0a, error: 0x0b };
const NAME = Object.fromEntries(Object.entries(FRAME).map(([k, v]) => [v, k]));
const enc = (type, header, payload = new Uint8Array(0)) => {
  const h = new TextEncoder().encode(JSON.stringify(header));
  const out = new Uint8Array(5 + h.length + payload.length);
  out[0] = type;
  new DataView(out.buffer).setUint32(1, h.length, true);
  out.set(h, 5);
  out.set(payload, 5 + h.length);
  return out;
};
const dec = (bytes) => {
  const b = new Uint8Array(bytes);
  const len = new DataView(b.buffer, b.byteOffset).getUint32(1, true);
  return { type: b[0], header: JSON.parse(new TextDecoder().decode(b.subarray(5, 5 + len))), payload: b.subarray(5 + len) };
};
/** Decode a GET /rows body: u32-LE length-prefixed frames. */
const decRows = (buf) => {
  const b = new Uint8Array(buf);
  const view = new DataView(b.buffer, b.byteOffset);
  const out = [];
  for (let off = 0; off < b.length; ) {
    const len = view.getUint32(off, true);
    out.push(dec(b.subarray(off + 4, off + 4 + len)));
    off += 4 + len;
  }
  return out;
};

// ── tiny WS client with a frame inbox ───────────────────────────────────────
class Client {
  constructor(device, token, chatId = chat, org = ORG) { Object.assign(this, { device, token, chatId, org }); this.inbox = []; this.closed = null; }
  async connect(epoch) {
    const q = epoch === undefined ? "" : `&epoch=${epoch}`;
    this.ws = new WebSocket(`${wsBase}/draft/${this.org}/${this.chatId}/ws?device=${this.device}&token=${this.token}${q}`);
    this.ws.binaryType = "arraybuffer";
    this.ws.onmessage = (ev) => this.inbox.push(dec(ev.data));
    this.ws.onclose = (ev) => { this.closed = { code: ev.code, reason: ev.reason }; };
    await new Promise((res, rej) => { this.ws.onopen = res; this.ws.onerror = () => rej(new Error("ws connect failed")); });
  }
  send(type, header, payload) { this.ws.send(enc(type, header, payload)); }
  async next(type, timeoutMs = 8000) {
    const start = Date.now();
    for (;;) {
      const i = this.inbox.findIndex((f) => f.type === type);
      if (i >= 0) return this.inbox.splice(i, 1)[0];
      if (this.closed) throw new Error(`socket closed (${this.closed.code}) while waiting for ${NAME[type]}`);
      if (Date.now() - start > timeoutMs) throw new Error(`timeout waiting for ${NAME[type]}; inbox=[${this.inbox.map((f) => NAME[f.type])}]`);
      await new Promise((r) => setTimeout(r, 25));
    }
  }
  async waitClose(timeoutMs = 8000) {
    const start = Date.now();
    while (!this.closed) {
      if (Date.now() - start > timeoutMs) throw new Error("timeout waiting for close");
      await new Promise((r) => setTimeout(r, 25));
    }
    return this.closed;
  }
  async hello(cursor = 0) { this.send(FRAME.hello, { cursor, device: this.device }); return this.next(FRAME.state); }
}
/** True iff the WS handshake is refused (non-101). */
const wsRefused = async (token, epoch, chatId = chat) => {
  const c = new Client("probe", token, chatId);
  try { await c.connect(epoch); c.ws.close(); return false; } catch { return true; }
};

const http = (path, { method = "GET", token = alice, headers = {}, body } = {}) =>
  fetch(`${base}${path}`, { method, headers: { authorization: `Bearer ${token}`, ...headers }, body });
const d = (action, query = "", c = chat) => `/draft/${ORG}/${c}/${action}${query}`;
const pushHttp = (epoch, batchId, bytes, device = "httpdev", token = alice, c = chat) =>
  http(d("rows", `?epoch=${epoch}&device=${device}&batchId=${batchId}`, c), { method: "POST", token, body: bytes });

let pass = 0, fail = 0;
const results = [];
const check = (name, cond, detail = "") => {
  if (cond) { pass++; results.push(`  ok  ${name}`); }
  else { fail++; results.push(`FAIL  ${name}${detail ? ` — ${detail}` : ""}`); }
};
const eqBytes = (a, b) => a.length === b.length && a.every((v, i) => v === b[i]);

// 1. Worker gates
{
  const h = await (await fetch(`${base}/health`)).json();
  check("health: dev auth mode", h.ok === true && h.auth === "dev");
  const unauth = await fetch(`${base}${d("epoch")}`);
  check("unauthenticated → 401", unauth.status === 401, `got ${unauth.status}`);
  const wrongOrg = await http(`/draft/org2/${chat}/epoch`);
  check("org claim != URL org → 403", wrongOrg.status === 403, `got ${wrongOrg.status}`);
  const badId = await http(d("epoch", "", "bad!id"));
  check("bad chat id → 400", badId.status === 400, `got ${badId.status}`);
  const noUpgrade = await http(d("ws", "?epoch=1"));
  check("ws without upgrade header → 426", noUpgrade.status === 426, `got ${noUpgrade.status}`);
  const unknown = await http(d("bogus"));
  check("unknown route → 404", unknown.status === 404, `got ${unknown.status}`);
  const tail = await http(d("tail"));
  check("no /tail sidecar → 404", tail.status === 404, `got ${tail.status}`);
  const badMethod = await http(d("discard", "?epoch=1"), { method: "GET" });
  check("wrong method → 404", badMethod.status === 404, `got ${badMethod.status}`);
  const ep = await (await http(d("epoch"))).json();
  check("fresh room epoch is 1", ep.epoch === 1, JSON.stringify(ep));
}

// 2. Epoch gate
{
  for (const q of ["", "?epoch=2", "?epoch=junk"]) {
    const rows = await http(d("rows", q));
    const body = await rows.json();
    check(`GET rows ${q || "(no epoch)"} → 409 epoch_mismatch`, rows.status === 409 && body.error === "epoch_mismatch" && body.epoch === 1, `${rows.status} ${JSON.stringify(body)}`);
  }
  const cp = await http(d("checkpoint", "?seqCovered=0&epoch=2"), { method: "POST", body: new Uint8Array([1]) });
  check("POST checkpoint wrong epoch → 409", cp.status === 409, `got ${cp.status}`);
  await cp.arrayBuffer();
  const push = await pushHttp(2, "nope", new Uint8Array([1]));
  check("POST rows wrong epoch → 409", push.status === 409, `got ${push.status}`);
  await push.arrayBuffer();
  check("ws upgrade with no epoch refused", await wsRefused(alice, undefined));
  check("ws upgrade with wrong epoch refused", await wsRefused(alice, 7));
}

// 3. Two devices: hello / state / push / ack / relay / backfill
const devA = new Client("devA", alice);
const devB = new Client("devB", alice);
await devA.connect(1);
await devB.connect(1);
{
  const stA = await devA.hello();
  const stB = await devB.hello();
  check("hello → state on fresh draft", stA.header.headSeq === 0 && stB.header.rowCount === 0 && stA.payload.length === 0, JSON.stringify(stA.header));
  const row = new Uint8Array(randomBytes(500));
  devA.send(FRAME.push, { batchId: "a-1" }, row);
  const ack = await devA.next(FRAME.ack);
  check("push acked seq=1", ack.header.seq === 1 && ack.header.dup === false && ack.header.batchId === "a-1", JSON.stringify(ack.header));
  const relayed = await devB.next(FRAME.row);
  check("row relayed to second device", relayed.header.seq === 1 && relayed.header.device === "devA" && eqBytes(relayed.payload, row), JSON.stringify(relayed.header));
  check("sender does not get its own row back", devA.inbox.every((f) => f.type !== FRAME.row));
  devA.send(FRAME.push, { batchId: "a-1" }, row);
  const dup = await devA.next(FRAME.ack);
  check("dup batchId → dup:true, original seq", dup.header.dup === true && dup.header.seq === 1);
  const rowB = new Uint8Array(randomBytes(300));
  const httpAck = await (await pushHttp(1, "http-1", rowB, "devH")).json();
  const relayedHttp = await devA.next(FRAME.row);
  check("HTTP POST /rows acked and relayed to sockets", httpAck.seq === 2 && relayedHttp.header.device === "devH" && eqBytes(relayedHttp.payload, rowB), JSON.stringify(httpAck));
  devB.inbox.length = 0; // drop the live relay of row 2; test the backfill path alone
  devB.send(FRAME.rowsReq, { after: 0 });
  const r1 = await devB.next(FRAME.row);
  const r2 = await devB.next(FRAME.row);
  const done = await devB.next(FRAME.rowsDone);
  check("rowsReq backfill in order + rowsDone", r1.header.seq === 1 && r2.header.seq === 2 && done.header.headSeq === 2);
  devA.send(FRAME.probe, {});
  check("probe → probeOk", (await devA.next(FRAME.probeOk)).header.headSeq === 2);
  const pull = decRows(await (await http(d("rows", "?epoch=1&after=1"))).arrayBuffer());
  check("GET /rows?after=1 → state, row 2, rowsDone", pull.length === 3 && pull[0].type === FRAME.state && pull[1].header.seq === 2 && pull[2].type === FRAME.rowsDone, pull.map((f) => NAME[f.type]).join(","));

  const cpBody = new Uint8Array(randomBytes(2000));
  const cp = await http(d("checkpoint", "?epoch=1&seqCovered=2"), { method: "POST", headers: { "x-chat2-frontier": Buffer.from([1, 2, 3]).toString("base64") }, body: cpBody });
  const cpJson = await cp.json();
  check("checkpoint commit prunes covered rows", cp.status === 200 && cpJson.seqFloor === 2 && cpJson.pruned === 2, JSON.stringify(cpJson));
  const got = await http(d("checkpoint", "?epoch=1"), { headers: { range: "bytes=1000-" } });
  check("GET checkpoint Range → 206 correct slice", got.status === 206 && eqBytes(new Uint8Array(await got.arrayBuffer()), cpBody.subarray(1000)), `${got.status}`);
}

// 4. Limits
{
  devA.send(FRAME.push, { batchId: "big-ws" }, new Uint8Array(64 * 1024 + 1));
  const err = await devA.next(FRAME.error);
  check("WS row > 64 KiB → too_large error with batchId", err.header.code === "too_large" && err.header.batchId === "big-ws", JSON.stringify(err.header));
  const big = await pushHttp(1, "big-http", new Uint8Array(64 * 1024 + 1));
  check("HTTP row > 64 KiB → 413", big.status === 413, `got ${big.status}`);
  await big.arrayBuffer();
  const bigCp = await http(d("checkpoint", "?epoch=1&seqCovered=2"), { method: "POST", headers: { "x-chat2-frontier": "AQ==" }, body: new Uint8Array(256 * 1024 + 1) });
  check("checkpoint > 256 KiB → 413", bigCp.status === 413, `got ${bigCp.status}`);
  await bigCp.arrayBuffer();
  const q = new Client("devQ", alice);
  await q.connect(1);
  await q.hello();
  let quotaAt = -1, acks = 0;
  for (let i = 0; i < 305 && quotaAt < 0; i++) {
    q.send(FRAME.push, { batchId: `q-${i}` }, new Uint8Array([1, 2, 3]));
    const start = Date.now();
    for (;;) {
      const a = q.inbox.findIndex((f) => f.type === FRAME.ack);
      const e = q.inbox.findIndex((f) => f.type === FRAME.error);
      if (a >= 0) { q.inbox.splice(a, 1); acks++; break; }
      if (e >= 0) { quotaAt = q.inbox.splice(e, 1)[0].header.code === "quota" ? i : -2; break; }
      if (Date.now() - start > 8000) throw new Error("quota loop timeout");
      await new Promise((r) => setTimeout(r, 5));
    }
  }
  check("push quota trips after 300 pushes/min", quotaAt === 300 && acks === 300, `quotaAt=${quotaAt} acks=${acks}`);
  q.ws.close();
}

// 5. Isolation between users
{
  const bobEpoch = await (await http(d("epoch"), { token: bob })).json();
  check("bob's room for the same chat id starts at epoch 1, untouched", bobEpoch.epoch === 1);
  const bobStats = await (await http(d("stats"), { token: bob })).json();
  check("bob sees none of alice's rows", bobStats.headSeq === 0 && bobStats.rowCount === 0 && bobStats.checkpointSize === 0, JSON.stringify(bobStats));
  const aliceBefore = await (await http(d("stats"))).json();
  const bobPush = await (await pushHttp(1, "bob-1", new Uint8Array([9]), "bobdev", bob)).json();
  check("bob's push starts at seq 1 in his own room", bobPush.seq === 1, JSON.stringify(bobPush));
  const aliceStats = await (await http(d("stats"))).json();
  check("bob's push did not reach alice's room", aliceStats.epoch === 1 && aliceStats.headSeq === aliceBefore.headSeq, JSON.stringify({ epoch: aliceStats.epoch, headSeq: aliceStats.headSeq }));
  const bobSock = new Client("bobdev", bob);
  await bobSock.connect(1);
  await bobSock.hello();
  devA.inbox.length = 0;
  bobSock.send(FRAME.push, { batchId: "bob-ws" }, new Uint8Array([5, 5]));
  await bobSock.next(FRAME.ack);
  await new Promise((r) => setTimeout(r, 300));
  check("bob's ws push is not relayed to alice's sockets", devA.inbox.every((f) => f.type !== FRAME.row));

  // 6. Discard
  const badDiscard = await http(d("discard"), { method: "POST" });
  check("discard without epoch → 400", badDiscard.status === 400, `got ${badDiscard.status}`);
  const discard = await http(d("discard", "?epoch=1"), { method: "POST" });
  const dj = await discard.json();
  check("discard current epoch → {epoch:2, discarded:true}", discard.status === 200 && dj.epoch === 2 && dj.discarded === true, JSON.stringify(dj));
  const cA = await devA.waitClose();
  const cB = await devB.waitClose();
  check("discard closes open sockets with 4411 'draft discarded'", cA.code === 4411 && cB.code === 4411 && cA.reason === "draft discarded", `${JSON.stringify(cA)} ${JSON.stringify(cB)}`);
  const stale = await (await http(d("discard", "?epoch=1"), { method: "POST" })).json();
  check("stale discard is a no-op → {epoch:2, discarded:false}", stale.epoch === 2 && stale.discarded === false, JSON.stringify(stale));
  const old = await http(d("rows", "?epoch=1"));
  check("old epoch rejected after discard", old.status === 409 && (await old.json()).epoch === 2);
  check("ws redial on old epoch refused", await wsRefused(alice, 1));
  const stats = await (await http(d("stats"))).json();
  check("discard wiped rows and checkpoint", stats.epoch === 2 && stats.headSeq === 0 && stats.rowCount === 0 && stats.checkpointSize === 0, JSON.stringify(stats));
  const gone = await http(d("checkpoint", "?epoch=2"));
  check("checkpoint gone after discard → 404", gone.status === 404, `got ${gone.status}`);
  await gone.arrayBuffer();
  const re = new Client("devA", alice);
  await re.connect(2);
  const st = await re.hello();
  check("redial on new epoch → empty state", st.header.headSeq === 0 && st.header.rowCount === 0 && st.payload.length === 0, JSON.stringify(st.header));
  re.ws.close();
  const bobAfter = await (await http(d("stats"), { token: bob })).json();
  check("alice's discard left bob's room intact", bobAfter.epoch === 1 && bobAfter.rowCount === 2, JSON.stringify({ epoch: bobAfter.epoch, rowCount: bobAfter.rowCount }));
  check("bob's socket survived alice's discard", bobSock.closed === null);
  bobSock.ws.close();
}

console.log(`\ndraft E2E vs ${base}\nchat: ${chat}\n`);
console.log(results.join("\n"));
console.log(`\n${pass} passed, ${fail} failed`);
process.exit(fail > 0 ? 1 : 0);
