/**
 * DraftRoom — one Durable Object per (org, user, chat) composer draft
 * (`draft1/{orgId}/{userId}/{chatId}`, docs/draft-sync.md §1). A tiny Loro doc
 * shared by one user's devices, relayed as the same dumb authenticated row log
 * ChatRoom serves (chat-frames.ts / chat-log.ts / blobs.ts, unchanged): the DO
 * never parses a CRDT byte.
 *
 * What differs from ChatRoom:
 *  - No owner claim. The Worker names the room after the verified user id, so
 *    the DO only trusts AUTH_USER_HEADER.
 *  - EPOCH. `meta.epoch` (starts at 1). ws / rows / checkpoint require
 *    `?epoch=N` to equal the current epoch (409 `epoch_mismatch` otherwise;
 *    a websocket upgrade is refused with the same 409).
 *  - DISCARD (`POST /discard?epoch=N`) is how a sent draft leaves no history:
 *    every row, checkpoint blob and meta key is deleted, the epoch becomes
 *    N + 1 and every socket is closed with 4411. A stale N is a no-op.
 *  - IDLE EXPIRY. Each accepted write re-arms an alarm 30 days out; when it
 *    fires on a room that has been idle that long, all storage is deleted.
 *  - Small limits: 64 KiB rows, 256 KiB checkpoint.
 *  - No /tail, /diff, /reset sidecars and no R2 backup alarm.
 *
 * Hibernation discipline as ChatRoom: no wall-clock timers; ping/pong rides
 * the runtime auto-response pair.
 */
import { createBlobStore, type BlobStore } from "./blobs";
import {
  appendRow,
  CHECKPOINT_BLOB,
  commitCheckpoint,
  ensureChatLog,
  FRONTIER_BLOB,
  getMeta,
  headSeq,
  logStats,
  rowsAfter,
  setMeta
} from "./chat-log";
import { decodeFrame, encodeFrame, FRAME } from "./chat-frames";
import {
  CLOSE_DRAFT_DISCARDED,
  CLOSE_DRAFT_DISCARDED_REASON,
  DRAFT_IDLE_TTL_MS,
  INITIAL_EPOCH,
  isIdleExpired,
  MAX_CHECKPOINT_BYTES,
  MAX_ROW_BYTES,
  parseEpoch
} from "./draft-protocol";
import { AUTH_USER_HEADER, type Env } from "./env";

export {
  CLOSE_DRAFT_DISCARDED,
  DRAFT_IDLE_TTL_MS,
  MAX_CHECKPOINT_BYTES,
  MAX_ROW_BYTES
} from "./draft-protocol";

/** Inbound frame budget: one pushed row (+ header slack). */
const MAX_FRAME_BYTES = MAX_ROW_BYTES + 8192;
/** Presence beats older than this are swept before relay/stats. */
const PRESENCE_TTL_MS = 30_000;
/** Per-device push quota, rolling window (in-memory; resets on hibernation —
 * it contains a runaway client loop, it does not meter honest traffic). */
const QUOTA_WINDOW_MS = 60_000;
const QUOTA_MAX_PUSHES = 300;
const QUOTA_MAX_BYTES = 8 * 1024 * 1024;

interface SocketState {
  userId: string;
  device: string;
  /** The room epoch this socket joined under. */
  epoch: number;
  /** Set once a valid hello established the session. */
  ready?: boolean;
}

interface PushOutcome {
  ok: number;
  rejected: number;
  lastOkAt: number;
}

interface QuotaWindow {
  since: number;
  pushes: number;
  bytes: number;
}

export class DraftRoom implements DurableObject {
  private readonly ctx: DurableObjectState;
  private blobs: BlobStore;
  /** device → last presence beat (epoch ms). Memory-only by construction. */
  private readonly presence = new Map<string, number>();
  /** device → rolling push quota window. Memory-only. */
  private readonly quotas = new Map<string, QuotaWindow>();

  constructor(ctx: DurableObjectState, _env: Env) {
    this.ctx = ctx;
    ensureChatLog(ctx.storage.sql);
    this.blobs = createBlobStore(ctx.storage.sql);
    ctx.setWebSocketAutoResponse(new WebSocketRequestResponsePair("ping", "pong"));
  }

  // ── HTTP surface (only reachable through the authed Worker) ──────────────

  async fetch(request: Request): Promise<Response> {
    const url = new URL(request.url);
    const userId = request.headers.get(AUTH_USER_HEADER);
    if (!userId) return json({ error: "unauthenticated" }, 401);
    const sql = this.ctx.storage.sql;
    // First contact arms the idle-expiry alarm, so a chat that is only ever opened (never typed
    // into) does not leave an empty room behind forever.
    if (getMeta(sql, "lastWriteAt") == null) this.touch();
    const path = url.pathname;
    const method = request.method;

    if (path === "/epoch" && method === "GET") return json({ epoch: this.epoch() });

    if (path === "/discard" && method === "POST") {
      const requested = parseEpoch(url.searchParams.get("epoch"));
      if (requested === undefined) return json({ error: "bad_epoch" }, 400);
      const current = this.epoch();
      if (requested !== current) return json({ epoch: current, discarded: false });
      this.discard(current);
      return json({ epoch: current + 1, discarded: true });
    }

    if (path === "/stats" && method === "GET") {
      this.sweepPresence();
      const lastWriteAt = Number(getMeta(sql, "lastWriteAt") ?? "0");
      return json({
        ...logStats(sql),
        epoch: this.epoch(),
        connectedSockets: this.ctx.getWebSockets().length,
        presence: Object.fromEntries(this.presence),
        pushOutcomes: JSON.parse(getMeta(sql, "pushOutcomes") ?? "{}") as Record<
          string,
          PushOutcome
        >,
        lastWriteAt,
        idleExpiresAt: lastWriteAt > 0 ? lastWriteAt + DRAFT_IDLE_TTL_MS : 0
      });
    }

    const known =
      (path === "/ws" && method === "GET") ||
      (path === "/checkpoint" && (method === "GET" || method === "POST")) ||
      (path === "/rows" && (method === "GET" || method === "POST"));
    if (!known) return json({ error: "not found" }, 404);

    // Every stateful route is epoch-gated; a stale device can never push into
    // (or read) a newer epoch. This also runs BEFORE the websocket upgrade.
    const epoch = this.epoch();
    if (parseEpoch(url.searchParams.get("epoch")) !== epoch) {
      return json({ error: "epoch_mismatch", epoch }, 409);
    }

    if (path === "/ws") {
      const device = url.searchParams.get("device") ?? "";
      const pair = new WebSocketPair();
      this.ctx.acceptWebSocket(pair[1]);
      const state: SocketState = { userId, device, epoch };
      pair[1].serializeAttachment(state);
      return new Response(null, { status: 101, webSocket: pair[0] });
    }

    if (path === "/checkpoint" && method === "POST") {
      const seqCovered = Number(url.searchParams.get("seqCovered") ?? "");
      if (!Number.isInteger(seqCovered) || seqCovered < 0) {
        return json({ error: "bad_seq_covered" }, 400);
      }
      const frontier = decodeBase64(request.headers.get("x-chat2-frontier") ?? "");
      if (frontier === undefined) return json({ error: "bad_frontier" }, 400);
      if (frontier.byteLength === 0 && seqCovered > 0) {
        return json({ error: "bad_frontier", message: "empty frontier on a content checkpoint" }, 400);
      }
      const declared = Number(request.headers.get("content-length") ?? "0");
      if (declared > MAX_CHECKPOINT_BYTES) return json({ error: "too_large" }, 413);
      const body = new Uint8Array(await request.arrayBuffer());
      if (body.byteLength > MAX_CHECKPOINT_BYTES) return json({ error: "too_large" }, 413);
      const outcome = commitCheckpoint(sql, this.blobs, seqCovered, frontier, body, Date.now());
      if (!outcome.ok) return json({ error: outcome.error }, 409);
      this.touch();
      return json({ ok: true, seqFloor: outcome.seqFloor, pruned: outcome.pruned });
    }

    if (path === "/checkpoint") {
      const bytes = this.blobs.get(CHECKPOINT_BLOB);
      if (!bytes) return json({ error: "not_found" }, 404);
      // Range-resumable (bytes=N- only), same as chat2.
      const range = parseRangeStart(request.headers.get("range"));
      if (range !== null && range >= bytes.byteLength) {
        return new Response(null, {
          status: 416,
          headers: { "content-range": `bytes */${bytes.byteLength}` }
        });
      }
      const body = range !== null ? bytes.subarray(range) : bytes;
      const headers = new Headers({
        "content-type": "application/octet-stream",
        "content-length": String(body.byteLength),
        "accept-ranges": "bytes",
        "x-chat2-checkpoint-seq": getMeta(sql, "checkpointSeq") ?? "0"
      });
      if (range !== null) {
        headers.set(
          "content-range",
          `bytes ${range}-${bytes.byteLength - 1}/${bytes.byteLength}`
        );
      }
      return new Response(body, { status: range !== null ? 206 : 200, headers });
    }

    if (path === "/rows" && method === "GET") {
      // Same length-prefixed frame body as chat2: state, rows after `?after=`,
      // rowsDone. Drafts are tiny, so the whole tail always fits.
      const afterRaw = Number(url.searchParams.get("after") ?? "0");
      const after = Number.isInteger(afterRaw) && afterRaw >= 0 ? afterRaw : 0;
      const device = url.searchParams.get("device") ?? "";
      const exclude =
        url.searchParams.get("excludeOwn") === "1" && device !== "" ? device : undefined;
      const frames: Uint8Array[] = [this.stateFrame()];
      for (const row of rowsAfter(sql, after, exclude)) {
        frames.push(
          encodeFrame(
            FRAME.row,
            { seq: row.seq, device: row.device, batchId: row.batchId },
            row.bytes
          )
        );
      }
      frames.push(encodeFrame(FRAME.rowsDone, { headSeq: headSeq(sql) }));
      const total = frames.reduce((n, f) => n + 4 + f.length, 0);
      const body = new Uint8Array(total);
      const view = new DataView(body.buffer);
      let off = 0;
      for (const f of frames) {
        view.setUint32(off, f.length, true);
        body.set(f, off + 4);
        off += 4 + f.length;
      }
      return new Response(body, {
        headers: { "content-type": "application/octet-stream" }
      });
    }

    // POST /rows: push over plain HTTPS, batchId-deduped.
    const device = url.searchParams.get("device") ?? "";
    const batchId = url.searchParams.get("batchId") ?? "";
    if (batchId === "" || batchId.length > 128) {
      this.recordPush(device, false);
      return json({ error: "bad_push" }, 400);
    }
    const declared = Number(request.headers.get("content-length") ?? "0");
    if (declared > MAX_ROW_BYTES + 4096) {
      this.recordPush(device, false);
      return json({ error: "too_large" }, 413);
    }
    const payload = new Uint8Array(await request.arrayBuffer());
    if (payload.byteLength > MAX_ROW_BYTES) {
      this.recordPush(device, false);
      return json({ error: "too_large" }, 413);
    }
    if (!this.admitQuota(device, payload.byteLength)) {
      this.recordPush(device, false);
      return json({ error: "quota" }, 429);
    }
    const outcome = appendRow(sql, device, batchId, payload, Date.now());
    if (!outcome.ok) {
      this.recordPush(device, false);
      return json({ error: outcome.error }, outcome.error === "too_large" ? 413 : 400);
    }
    this.recordPush(device, true);
    if (!outcome.dup) {
      this.touch();
      for (const socket of this.ctx.getWebSockets()) {
        const socketState = socket.deserializeAttachment() as SocketState | null;
        if (!socketState?.ready) continue;
        send(socket, FRAME.row, { seq: outcome.seq, device, batchId }, payload);
      }
    }
    return json({ batchId, seq: outcome.seq, dup: outcome.dup });
  }

  // ── WebSocket protocol (binary frames, chat-frames.ts) ───────────────────

  async webSocketMessage(ws: WebSocket, message: ArrayBuffer | string): Promise<void> {
    if (typeof message === "string") {
      ws.close(1003, "binary frames only");
      return;
    }
    if (message.byteLength > MAX_FRAME_BYTES) {
      ws.close(1009, "frame too large");
      return;
    }
    const frame = decodeFrame(new Uint8Array(message));
    const state = ws.deserializeAttachment() as SocketState;
    // Belt and braces: discard closes every socket, so an open socket is
    // always current; a stale one (e.g. survivor of a race) must never write.
    if (state.epoch !== this.epoch()) {
      ws.close(CLOSE_DRAFT_DISCARDED, CLOSE_DRAFT_DISCARDED_REASON);
      return;
    }
    if (!frame) {
      send(ws, FRAME.error, { code: "bad_frame", message: "malformed frame" });
      return;
    }
    switch (frame.type) {
      case FRAME.hello:
        this.handleHello(ws, state, frame.header);
        return;
      case FRAME.rowsReq:
        this.handleRowsReq(ws, state, frame.header);
        return;
      case FRAME.push:
        this.handlePush(ws, state, frame.header, frame.payload);
        return;
      case FRAME.presence:
        this.handlePresence(ws, state, frame.header, frame.payload);
        return;
      case FRAME.probe:
        send(ws, FRAME.probeOk, { headSeq: headSeq(this.ctx.storage.sql) });
        return;
      default:
        send(ws, FRAME.error, { code: "bad_frame", message: `unexpected type ${frame.type}` });
    }
  }

  async webSocketClose(): Promise<void> {
    /* nothing buffered; rows are written synchronously on push */
  }

  async webSocketError(): Promise<void> {
    /* ditto */
  }

  private stateFrame(): Uint8Array {
    const stats = logStats(this.ctx.storage.sql);
    const frontier = this.blobs.get(FRONTIER_BLOB) ?? new Uint8Array(0);
    return encodeFrame(
      FRAME.state,
      {
        headSeq: stats.headSeq,
        seqFloor: stats.seqFloor,
        checkpointSeq: stats.checkpointSeq,
        checkpointSize: stats.checkpointSize,
        rowCount: stats.rowCount,
        rowBytes: stats.rowBytes
      },
      frontier
    );
  }

  private handleHello(ws: WebSocket, state: SocketState, header: Record<string, unknown>): void {
    if (typeof header.device === "string" && header.device.length > 0) {
      state.device = header.device;
    }
    state.ready = true;
    ws.serializeAttachment(state);
    const frame = this.stateFrame();
    ws.send(frame.buffer.slice(frame.byteOffset, frame.byteOffset + frame.byteLength) as ArrayBuffer);
  }

  private handleRowsReq(ws: WebSocket, state: SocketState, header: Record<string, unknown>): void {
    if (!state.ready) {
      send(ws, FRAME.error, { code: "hello_first", message: "rows before hello" });
      return;
    }
    const after = typeof header.after === "number" && header.after >= 0 ? header.after : 0;
    const exclude = header.excludeOwn === true ? state.device : undefined;
    const sql = this.ctx.storage.sql;
    for (const row of rowsAfter(sql, after, exclude)) {
      send(ws, FRAME.row, { seq: row.seq, device: row.device, batchId: row.batchId }, row.bytes);
    }
    send(ws, FRAME.rowsDone, { headSeq: headSeq(sql) });
  }

  private handlePush(
    ws: WebSocket,
    state: SocketState,
    header: Record<string, unknown>,
    payload: Uint8Array
  ): void {
    const batchId = typeof header.batchId === "string" ? header.batchId : "";
    if (!state.ready || batchId === "" || batchId.length > 128) {
      this.recordPush(state.device, false);
      send(ws, FRAME.error, { code: "bad_push", message: "hello first / malformed push", batchId });
      return;
    }
    if (payload.byteLength > MAX_ROW_BYTES) {
      this.recordPush(state.device, false);
      send(ws, FRAME.error, { code: "too_large", message: "push rejected: too_large", batchId });
      return;
    }
    if (!this.admitQuota(state.device, payload.byteLength)) {
      this.recordPush(state.device, false);
      send(ws, FRAME.error, { code: "quota", message: "per-device push quota exceeded", batchId });
      return;
    }
    const sql = this.ctx.storage.sql;
    const outcome = appendRow(sql, state.device, batchId, payload, Date.now());
    if (!outcome.ok) {
      this.recordPush(state.device, false);
      send(ws, FRAME.error, {
        code: outcome.error,
        message: `push rejected: ${outcome.error}`,
        batchId
      });
      return;
    }
    this.recordPush(state.device, true);
    if (!outcome.dup) {
      this.touch();
      for (const socket of this.ctx.getWebSockets()) {
        if (socket === ws) continue;
        const socketState = socket.deserializeAttachment() as SocketState | null;
        if (!socketState?.ready) continue;
        send(socket, FRAME.row, { seq: outcome.seq, device: state.device, batchId }, payload);
      }
    }
    send(ws, FRAME.ack, { batchId, seq: outcome.seq, dup: outcome.dup });
  }

  private handlePresence(
    ws: WebSocket,
    state: SocketState,
    header: Record<string, unknown>,
    payload: Uint8Array
  ): void {
    if (!state.ready || state.device === "") return;
    const at = typeof header.at === "number" ? header.at : Date.now();
    this.presence.set(state.device, at);
    this.sweepPresence();
    for (const socket of this.ctx.getWebSockets()) {
      if (socket === ws) continue;
      const socketState = socket.deserializeAttachment() as SocketState | null;
      if (!socketState?.ready) continue;
      send(socket, FRAME.presence, { device: state.device, at }, payload);
    }
  }

  private sweepPresence(): void {
    const horizon = Date.now() - PRESENCE_TTL_MS;
    for (const [device, at] of this.presence) {
      if (at < horizon) this.presence.delete(device);
    }
  }

  // ── epoch / discard / expiry ─────────────────────────────────────────────

  private epoch(): number {
    return parseEpoch(getMeta(this.ctx.storage.sql, "epoch")) ?? INITIAL_EPOCH;
  }

  /** Wipe rows, checkpoint blobs and meta, keep `epoch = current + 1`, and
   * drop every socket. One synchronous transaction: readers never see a
   * half-discarded room. */
  private discard(current: number): void {
    const sql = this.ctx.storage.sql;
    this.ctx.storage.transactionSync(() => {
      sql.exec("DELETE FROM rows");
      sql.exec("DELETE FROM blobs");
      sql.exec("DELETE FROM meta");
      setMeta(sql, "epoch", String(current + 1));
    });
    this.presence.clear();
    this.closeSockets();
    this.touch();
  }

  private closeSockets(): void {
    for (const ws of this.ctx.getWebSockets()) {
      try {
        ws.close(CLOSE_DRAFT_DISCARDED, CLOSE_DRAFT_DISCARDED_REASON);
      } catch {
        /* already gone */
      }
    }
  }

  /** An accepted write: record it and re-arm the idle alarm a full TTL out. */
  private touch(): void {
    const now = Date.now();
    setMeta(this.ctx.storage.sql, "lastWriteAt", String(now));
    void this.ctx.storage.setAlarm(now + DRAFT_IDLE_TTL_MS).catch(() => {
      /* the next write re-arms */
    });
  }

  /** Idle-expiry alarm. Fires 30 days after the last write; if a later write
   * moved the deadline (or the alarm is stale) re-arm instead of deleting. */
  async alarm(): Promise<void> {
    const sql = this.ctx.storage.sql;
    const lastWriteAt = Number(getMeta(sql, "lastWriteAt") ?? "0");
    if (!isIdleExpired(lastWriteAt, Date.now())) {
      await this.ctx.storage.setAlarm(lastWriteAt + DRAFT_IDLE_TTL_MS);
      return;
    }
    this.closeSockets();
    this.presence.clear();
    await this.ctx.storage.deleteAlarm();
    await this.ctx.storage.deleteAll();
    // The instance may outlive the wipe; recreate the (empty) schema so the
    // next request works. The epoch restarts at 1 — the room is gone.
    ensureChatLog(sql);
    this.blobs = createBlobStore(sql);
  }

  // ── quota / attribution ──────────────────────────────────────────────────

  /** Rolling per-device quota. True = admitted. */
  private admitQuota(device: string, bytes: number): boolean {
    const now = Date.now();
    const key = device === "" ? "(unknown)" : device;
    let window = this.quotas.get(key);
    if (!window || now - window.since > QUOTA_WINDOW_MS) {
      window = { since: now, pushes: 0, bytes: 0 };
      this.quotas.set(key, window);
    }
    window.pushes += 1;
    window.bytes += bytes;
    return window.pushes <= QUOTA_MAX_PUSHES && window.bytes <= QUOTA_MAX_BYTES;
  }

  private recordPush(device: string, ok: boolean): void {
    const sql = this.ctx.storage.sql;
    const key = device === "" ? "(unknown)" : device;
    const outcomes = JSON.parse(getMeta(sql, "pushOutcomes") ?? "{}") as Record<
      string,
      PushOutcome
    >;
    const entry = outcomes[key] ?? { ok: 0, rejected: 0, lastOkAt: 0 };
    if (ok) {
      entry.ok += 1;
      entry.lastOkAt = Date.now();
    } else {
      entry.rejected += 1;
    }
    outcomes[key] = entry;
    setMeta(sql, "pushOutcomes", JSON.stringify(outcomes));
  }
}

const send = (
  ws: WebSocket,
  type: (typeof FRAME)[keyof typeof FRAME],
  header: Record<string, unknown>,
  payload?: Uint8Array
): void => {
  try {
    const frame = encodeFrame(type, header, payload);
    ws.send(frame.buffer.slice(frame.byteOffset, frame.byteOffset + frame.byteLength) as ArrayBuffer);
  } catch {
    /* socket already gone; hibernation API cleans it up */
  }
};

const json = (value: unknown, status = 200): Response =>
  new Response(JSON.stringify(value), {
    status,
    headers: { "content-type": "application/json" }
  });

/** `bytes=N-` (open-ended resume) only; anything fancier is ignored → 200. */
const parseRangeStart = (header: string | null): number | null => {
  const match = header?.match(/^bytes=(\d+)-$/);
  if (!match) return null;
  const start = Number(match[1]);
  return Number.isSafeInteger(start) && start > 0 ? start : null;
};

/** Standard base64 (empty string ⇒ empty frontier). `undefined` = malformed. */
const decodeBase64 = (text: string): Uint8Array | undefined => {
  try {
    const bin = atob(text);
    const out = new Uint8Array(bin.length);
    for (let i = 0; i < bin.length; i++) out[i] = bin.charCodeAt(i);
    return out;
  } catch {
    return undefined;
  }
};
