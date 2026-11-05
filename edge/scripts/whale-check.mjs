// Regression check for the whale-session persistence bug (2026-08-05): a
// single Loro update larger than the DO SQLite ~2MB row cap used to import,
// ack Ok, and relay — while every flush() INSERT threw SQLITE_TOOBIG, so the
// room silently stopped persisting and each hibernation reverted it to stale
// state. update-log.ts now chunks oversized updates across continuation rows.
// Drives a >2MB single-update push through the WS fragment path against a
// running `wrangler dev` (AUTH_MODE=dev) and asserts the room stays healthy
// AND durable (rows or folded snapshot persisted, /stats + /tail 200).
//
// Usage: node scripts/whale-check.mjs [baseUrl] [payloadMB]
import { LoroDoc, LoroMap, LoroText } from "loro-crdt";
import { LoroWebsocketClient } from "loro-websocket";
import { LoroAdaptor } from "loro-adaptors/loro";
import { randomUUID } from "node:crypto";

const base = process.argv[2] ?? "http://127.0.0.1:27640";
const payloadMB = Number(process.argv[3] ?? "3");
const wsBase = base.replace(/^http/, "ws");
const token = "whale-user";
const chatId = `whale-${randomUUID().slice(0, 8)}`;

const log = (msg) => console.log(msg);
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const stats = async () => {
  const res = await fetch(`${base}/stats/${chatId}?token=${token}`);
  return { status: res.status, body: res.status === 200 ? await res.json() : await res.text() };
};

// ── host joins, then writes the whale in ONE commit: a single giant Loro
//    update, exactly the shape of a full-doc re-upload / bulk import ────────
const host = new LoroWebsocketClient({ url: `${wsBase}/session/${chatId}/ws?token=${token}` });
await host.waitConnected();
const hostAdaptor = new LoroAdaptor();
await host.join({ roomId: chatId, crdtAdaptor: hostAdaptor });
const hostDoc = hostAdaptor.getDoc();

hostDoc.getMap("meta").set("chatId", chatId);
hostDoc.getMap("meta").set("schemaVersion", 1);
const messages = hostDoc.getList("messages");
const chunk = "The quick brown fox jumps over the lazy dog. ".repeat(90); // ~4KB
const targetMsgs = Math.ceil((payloadMB * 1024 * 1024) / chunk.length);
let i = 0;
const { LoroList } = await import("loro-crdt");
for (; i < targetMsgs; i++) {
  const m = messages.insertContainer(messages.length, new LoroMap());
  m.set("id", `m${i}`);
  m.set("role", i % 2 ? "assistant" : "user");
  m.set("createdAt", 1754400000000 + i);
  m.set("deviceId", "whale-dev");
  const parts = m.setContainer("parts", new LoroList());
  const part = parts.insertContainer(0, new LoroMap());
  part.set("id", `p${i}`);
  part.set("kind", "text");
  part.setContainer("text", new LoroText()).insert(0, chunk);
}
hostDoc.commit(); // ONE update carrying everything
const updateBytes = hostDoc.export({ mode: "snapshot" }).length;
log(`host committed ${i} messages in one update (~${(updateBytes / 1e6).toFixed(2)}MB doc); pushing via WS fragments`);

await sleep(4000); // let the fragments upload + the DO import + ack
const s1 = await stats();
log(`stats immediately after push: ${s1.status} ${JSON.stringify(s1.body)}`);

await sleep(8000); // DO_FLUSH_MS elapsed — flush has now tried the big INSERT
const s2 = await stats();
log(`stats after flush window:    ${s2.status} ${JSON.stringify(s2.body)}`);

// ── reader joins fresh: does it get the messages live? ────────────────────
const reader = new LoroWebsocketClient({ url: `${wsBase}/session/${chatId}/ws?token=${token}` });
await reader.waitConnected();
const readerAdaptor = new LoroAdaptor();
await reader.join({ roomId: chatId, crdtAdaptor: readerAdaptor });
const readerDoc = readerAdaptor.getDoc();
const t0 = Date.now();
while (Date.now() - t0 < 20000) {
  if (readerDoc.getList("messages").length >= i) break;
  await sleep(100);
}
const initialReaderMessages = readerDoc.getList("messages").length;
log(`reader backfill: ${initialReaderMessages}/${i} messages (live doc still serves)`);

const tailRes = await fetch(`${base}/tail/${chatId}?token=${token}`);
const tailBody = tailRes.status === 200 ? await tailRes.json() : await tailRes.text();
log(`tail: ${tailRes.status}${tailRes.status !== 200 ? " " + String(tailBody).slice(0, 200) : ` (${tailBody.totalMessages} messages)`}`);

// ── fold backpressure: a >2MB log crosses COMPACT_LOG_BYTES on the same
//    flush that wrote its chunked rows, so foldLog must have collapsed the
//    log into the snapshot (force-trimming shallow via TRIM_FORCE_BYTES on a
//    chat room) and reset the byte accounting — then the room must keep
//    accepting, persisting, and serving writes on top of the folded doc. ────
const EXTRA = 40;
for (let j = 0; j < EXTRA; j++) {
  const m = messages.insertContainer(messages.length, new LoroMap());
  m.set("id", `m${i + j}`);
  m.set("role", j % 2 ? "assistant" : "user");
  m.set("createdAt", 1754400000000 + i + j);
  m.set("deviceId", "whale-dev");
  const parts = m.setContainer("parts", new LoroList());
  const part = parts.insertContainer(0, new LoroMap());
  part.set("id", `p${i + j}`);
  part.set("kind", "text");
  part.setContainer("text", new LoroText()).insert(0, `post-fold message ${j}`);
  hostDoc.commit(); // one update per message: exercises the row path, not another whale
}
await sleep(1500); // let the wave relay
const s3 = await stats(); // /stats flushes, so the wave's rows land durably now
log(`stats after post-fold wave:  ${s3.status} ${JSON.stringify(s3.body)}`);

// A fresh reader joining AFTER the fold+trim reads the shallow-aware §3.1
// backfill (the bug-#2 path: force-trimmed whale rooms used to serve fresh
// readers an EMPTY transcript with zero errors).
const reader2 = new LoroWebsocketClient({ url: `${wsBase}/session/${chatId}/ws?token=${token}` });
await reader2.waitConnected();
const reader2Adaptor = new LoroAdaptor();
await reader2.join({ roomId: chatId, crdtAdaptor: reader2Adaptor });
const reader2Doc = reader2Adaptor.getDoc();
const t1 = Date.now();
while (Date.now() - t1 < 20000) {
  if (reader2Doc.getList("messages").length >= i + EXTRA) break;
  await sleep(100);
}
log(`post-fold reader backfill: ${reader2Doc.getList("messages").length}/${i + EXTRA} messages`);

const tail2Res = await fetch(`${base}/tail/${chatId}?token=${token}`);
const tail2Body = tail2Res.status === 200 ? await tail2Res.json() : await tail2Res.text();

// ── verdict ───────────────────────────────────────────────────────────────
let failures = 0;
const check = (name, cond, detail = "") => {
  log(`${cond ? "PASS" : "FAIL"}  ${name}${detail ? " — " + detail : ""}`);
  if (!cond) failures++;
};
log("");
check("/stats stays 200 after the flush window", s2.status === 200, `status=${s2.status}`);
const durable =
  s2.status === 200 && (s2.body.updateRows > 0 || s2.body.snapshotBytes > 0);
check(
  "whale update became durable (log rows or folded snapshot)",
  durable,
  s2.status === 200 ? `updateRows=${s2.body.updateRows}, snapshotBytes=${s2.body.snapshotBytes}` : ""
);
check("reader received the whale live", initialReaderMessages === i);
check(
  "/tail materializes",
  tailRes.status === 200 && tailBody.totalMessages === i,
  `status=${tailRes.status}`
);

// Fold backpressure. The byte-budget assertions only hold when the payload
// actually crosses COMPACT_LOG_BYTES (2MB) — the default 3MB does.
if (payloadMB * 1024 * 1024 > 2 * 1024 * 1024) {
  check(
    "fold collapsed the chunked log into the snapshot",
    s2.status === 200 && s2.body.updateRows === 0 && s2.body.snapshotBytes > 0,
    s2.status === 200 ? `updateRows=${s2.body.updateRows}, snapshotBytes=${s2.body.snapshotBytes}` : ""
  );
  check(
    "fold reset the log byte accounting",
    s2.status === 200 && s2.body.updateLogBytes === 0,
    s2.status === 200 ? `updateLogBytes=${s2.body.updateLogBytes}` : ""
  );
  check(
    "force-trim engaged on the whale room",
    s2.status === 200 && !!s2.body.lastTrimAt,
    s2.status === 200 ? `lastTrimAt=${s2.body.lastTrimAt}` : ""
  );
}
check(
  "post-fold writes persist as fresh log rows",
  s3.status === 200 && s3.body.updateRows > 0,
  s3.status === 200 ? `updateRows=${s3.body.updateRows}` : `status=${s3.status}`
);
check("first reader stayed live across the fold", readerDoc.getList("messages").length === i + EXTRA);
check(
  "fresh post-fold reader gets the full transcript (shallow-aware backfill)",
  reader2Doc.getList("messages").length === i + EXTRA
);
check(
  "/tail reflects the post-fold wave",
  tail2Res.status === 200 && tail2Body.totalMessages === i + EXTRA,
  `status=${tail2Res.status}${tail2Res.status === 200 ? `, totalMessages=${tail2Body.totalMessages}` : ""}`
);
process.exit(failures === 0 ? 0 : 1);
