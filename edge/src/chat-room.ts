/**
 * ChatRoom — one Durable Object per chat session (`chat2/{chatId}`), the
 * dumb authenticated log relay replacing SessionRoom's loro-aware s2 rooms
 * (docs/chat2-sync.md workstream B). Modeled line-for-line on RegistryRoom,
 * NOT on SessionRoom: no loro-wasm import anywhere in this class.
 *
 * The DO's entire job: append opaque update blobs to a seq-ordered log,
 * relay them to live sockets, store one client-built checkpoint blob, and
 * serve both back. All CRDT semantics live in the clients. Cold start is a
 * table read — the s2 wedge class (CPU-limited export/replay in the DO)
 * cannot exist here by construction.
 *
 * Sidecars (`/tail`, `/diff`) are host-PUBLISHED and served verbatim: the DO
 * never materializes anything.
 *
 * Hibernation discipline: ZERO wall-clock timers; ping/pong rides the
 * auto-response pair; alarms retry accepted rows' host wakes and back up to R2.
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
  MAX_ROW_BYTES,
  rowsAfter,
  setMeta
} from "./chat-log";
import { decodeFrame, encodeFrame, FRAME } from "./chat-frames";
import {
  DEFAULT_PAGE_LIMIT,
  ensureTranscript,
  getOutput,
  headRev,
  ITEMS_BODY_CAP,
  itemsPage,
  itemsSince,
  MAX_OUTPUT_BYTES,
  PART_RE,
  parseItemKey,
  putItem,
  putOutput,
  resetTranscript,
  transcriptStats,
  validateItemKey
} from "./chat-transcript";
import { AUTH_USER_HEADER, type Env } from "./env";
import { meterSql, newSqlMeter, type SqlMeter } from "./sql-meter";
import {
  ensureChatWakes, finishChatWake, MAX_WAKE_ATTEMPTS, PERMANENT_WAKE_REJECTIONS,
  pendingChatWake, queueChatWake, retryChatWake, validWakeRoute
} from "./chat-wakes";

const DAY_MS = 24 * 60 * 60 * 1000;
/** Inbound frame budget: one pushed row (+ header slack). */
const MAX_FRAME_BYTES = MAX_ROW_BYTES + 8192;
/** Host-published sidecar budget (tail JSON / diff payload). */
const MAX_SIDECAR_BYTES = 4 * 1024 * 1024;
/** Existing long-running sessions exceed 16 MiB. Keep uploads bounded,
 * but allow their checkpoints to advance instead of stranding the row log. */
export const MAX_CHECKPOINT_BYTES = 32 * 1024 * 1024;
/** Presence beats older than this are swept before relay/stats. */
const PRESENCE_TTL_MS = 30_000;
/** Per-device push quota, rolling window (in-memory; resets on hibernation —
 * it exists to contain a runaway client loop, not to meter honest traffic). */
const QUOTA_WINDOW_MS = 60_000;
const QUOTA_MAX_PUSHES = 300;
const QUOTA_MAX_BYTES = 8 * 1024 * 1024;
/** chat3: one `POST /items` batch (the host sends ≤ 4 MiB). */
const MAX_ITEMS_BODY_BYTES = 8 * 1024 * 1024;
/** chat3 live cache bounds: one open part's frames, and the room's total.
 * Past them the oldest part is dropped — a viewer then waits for the next
 * `part` frame or the stored row, which is all a dropped live frame costs. */
const LIVE_PART_MAX_BYTES = 1024 * 1024;
const LIVE_TOTAL_MAX_BYTES = 8 * 1024 * 1024;
/** One live frame's payload (a part's JSON or an append). */
const MAX_LIVE_PAYLOAD_BYTES = 256 * 1024;

interface LivePart {
  frames: Uint8Array[];
  bytes: number;
}

interface SocketState {
  userId: string;
  device: string;
  /** Set once a valid hello established the session. */
  ready?: boolean;
  chatId?: string;
  hostDevice?: string;
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

export class ChatRoom implements DurableObject {
  private readonly ctx: DurableObjectState;
  private readonly env: Env;
  private readonly blobs: BlobStore;
  /** Every statement this room runs, metered with the runtime's own billing
   * counters (sql-meter.ts) and reported in `/stats`. */
  private readonly sql: SqlStorage;
  private readonly meter: SqlMeter = newSqlMeter();
  /** chat3 tables exist (created on first transcript use: gen-2 rooms never
   * pay for them). */
  private transcriptReady = false;
  /** chat3 open parts, `ord:idx` → the live frames that rebuild it (a `part`
   * then its `append`s). Memory-only: a late viewer gets them after its
   * backfill; a stored row for the key supersedes and drops them. */
  private readonly live = new Map<string, LivePart>();
  private liveBytes = 0;
  /** device → last presence beat (epoch ms). Memory-only by construction. */
  private readonly presence = new Map<string, number>();
  /** device → rolling push quota window. Memory-only. */
  private readonly quotas = new Map<string, QuotaWindow>();
  private wakeFlight?: Promise<void>;

  constructor(ctx: DurableObjectState, env: Env) {
    this.ctx = ctx;
    this.env = env;
    this.sql = meterSql(ctx.storage.sql, this.meter);
    ensureChatLog(this.sql);
    ensureChatWakes(this.sql);
    this.blobs = createBlobStore(this.sql);
    // Runtime-answered keepalive; proves nothing about this DO's health.
    // Clients judge liveness by probe frames (same caveat as RegistryRoom).
    ctx.setWebSocketAutoResponse(new WebSocketRequestResponsePair("ping", "pong"));
  }

  // ── HTTP surface (only reachable through the authed Worker) ──────────────

  async fetch(request: Request): Promise<Response> {
    const url = new URL(request.url);
    const userId = request.headers.get(AUTH_USER_HEADER);
    if (!userId) return json({ error: "unauthenticated" }, 401);

    const sql = this.sql;
    const owner = getMeta(sql, "owner");

    if (url.pathname === "/ws") {
      // Claim-on-first-join ownership, then owner-only forever (the s2
      // discipline: chat ids are client-minted, the first authed user to
      // dial one owns it).
      if (!owner) setMeta(sql, "owner", userId);
      else if (owner !== userId) return json({ error: "forbidden" }, 403);
      const device = url.searchParams.get("device") ?? "";
      const chatId = url.searchParams.get("chatId") ?? "";
      // An unroutable wake hint never refuses the push: the rows are the
      // user's message; only host discovery is skipped.
      const hint = url.searchParams.get("hostDevice") ?? "";
      const hostDevice = hint && validWakeRoute(chatId, hint) ? hint : "";
      const pair = new WebSocketPair();
      this.ctx.acceptWebSocket(pair[1]);
      const state: SocketState = { userId, device, chatId, hostDevice };
      pair[1].serializeAttachment(state);
      return new Response(null, { status: 101, webSocket: pair[0] });
    }

    if (url.pathname === "/checkpoint" && request.method === "POST") {
      if (!owner) setMeta(sql, "owner", userId);
      else if (owner !== userId) return json({ error: "forbidden" }, 403);
      const seqCovered = Number(url.searchParams.get("seqCovered") ?? "");
      if (!Number.isInteger(seqCovered) || seqCovered < 0) {
        return json({ error: "bad_seq_covered" }, 400);
      }
      const frontier = decodeBase64(request.headers.get("x-chat2-frontier") ?? "");
      if (frontier === undefined) return json({ error: "bad_frontier" }, 400);
      // A checkpoint that claims to cover rows must name its state: an empty
      // frontier label on a content-bearing checkpoint made every fresh
      // reader skip it and park all dependent rows invisibly ("Add Tweets"
      // incident, 2026-08-18). Empty stays legal only for seqCovered 0
      // (the M1 empty-doc seed).
      if (frontier.byteLength === 0 && seqCovered > 0) {
        return json({ error: "bad_frontier", message: "empty frontier on a content checkpoint" }, 400);
      }
      const body = new Uint8Array(await request.arrayBuffer());
      if (body.byteLength > MAX_CHECKPOINT_BYTES) return json({ error: "too_large" }, 413);
      const outcome = commitCheckpoint(sql, this.blobs, seqCovered, frontier, body, Date.now());
      if (!outcome.ok) return json({ error: outcome.error }, 409);
      await this.markBackupDirty();
      return json({ ok: true, seqFloor: outcome.seqFloor, pruned: outcome.pruned });
    }

    // Claim-on-first-contact for the HTTP surface too — same client-minted-id
    // discipline as /ws. The /rows twins predate the pull-first HTTPS
    // transport and 404'd an unclaimed room, which deadlocked a brand-new
    // chat on WS-hostile networks: the sender's push 404s, the host's pull
    // 404s, and the only claimants (WS join, checkpoint heal) never run.
    if (!owner) setMeta(sql, "owner", userId);
    else if (owner !== userId) return json({ error: "forbidden" }, 403);

    if (url.pathname === "/checkpoint" && request.method === "GET") {
      const bytes = this.blobs.get(CHECKPOINT_BLOB);
      if (!bytes) return json({ error: "not_found" }, 404);
      // Range-resumable (bytes=N- only): a 1MB load over a 1.2Mbps link that
      // dies at byte 800k resumes, where s2's export-per-join restarted.
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

    if (url.pathname === "/rows" && request.method === "GET") {
      // Pull over plain HTTPS: one GET collapses connect → hello → state →
      // rowsReq → backfill (4+ serial round trips on a WS, and impossible on
      // networks that strip the upgrade) into a single request. The body is
      // u32-LE length-prefixed frames — state (frontier payload), rows after
      // `?after=`, rowsDone — byte-identical frame encoding to the WS path,
      // so clients reuse their existing decoder.
      const afterRaw = Number(url.searchParams.get("after") ?? "0");
      const after = Number.isInteger(afterRaw) && afterRaw >= 0 ? afterRaw : 0;
      const device = url.searchParams.get("device") ?? "";
      const exclude =
        url.searchParams.get("excludeOwn") === "1" && device !== "" ? device : undefined;
      const stats = logStats(sql);
      const frontier = this.blobs.get(FRONTIER_BLOB) ?? new Uint8Array(0);
      const frames: Uint8Array[] = [
        encodeFrame(
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
        )
      ];
      // Response cap: the WS path streams; this buffers, so bound the body.
      // Past the cap the response ends WITHOUT rowsDone — clients apply what
      // arrived, their cursor advances per row, and the next pull resumes
      // from there (pagination by truncation).
      const ROWS_BODY_CAP = 4 * 1024 * 1024;
      let bodyBytes = 0;
      let truncated = false;
      for (const row of rowsAfter(sql, after, exclude)) {
        const frame = encodeFrame(
          FRAME.row,
          { seq: row.seq, device: row.device, batchId: row.batchId },
          row.bytes
        );
        bodyBytes += 4 + frame.length;
        if (bodyBytes > ROWS_BODY_CAP) {
          truncated = true;
          break;
        }
        frames.push(frame);
      }
      if (!truncated) {
        frames.push(encodeFrame(FRAME.rowsDone, { headSeq: headSeq(sql) }));
      }
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

    if (url.pathname === "/rows" && request.method === "POST") {
      // Push over plain HTTPS — the WS push's fallback twin for networks
      // where the upgrade never completes. batchId dedupe (UNIQUE column)
      // makes at-least-once delivery exact-once in effect.
      const device = url.searchParams.get("device") ?? "";
      const batchId = url.searchParams.get("batchId") ?? "";
      const chatId = url.searchParams.get("chatId") ?? "";
      const hint = url.searchParams.get("hostDevice") ?? "";
      const hostDevice = hint && validWakeRoute(chatId, hint) ? hint : "";
      if (batchId === "" || batchId.length > 128) {
        this.recordPush(device, false);
        return json({ error: "bad_push" }, 400);
      }
      // Pre-read cap (the WS runtime closes 1 MiB messages before the DO
      // runs; HTTP needs the explicit twin). appendRow re-checks post-read.
      const declared = Number(request.headers.get("content-length") ?? "0");
      if (declared > MAX_ROW_BYTES + 4096) {
        this.recordPush(device, false);
        return json({ error: "too_large" }, 413);
      }
      const payload = new Uint8Array(await request.arrayBuffer());
      if (!this.admitQuota(device, payload.byteLength)) {
        this.recordPush(device, false);
        return json({ error: "quota" }, 429);
      }
      const outcome = await this.acceptRow(device, batchId, payload, userId, chatId, hostDevice);
      if (!outcome.ok) {
        this.recordPush(device, false);
        return json({ error: outcome.error }, outcome.error === "too_large" ? 413 : 400);
      }
      this.recordPush(device, true);
      if (!outcome.dup) {
        // Live relay to every ready socket — a same-device socket would
        // re-import its own bytes as a Loro no-op, so no exclusion needed.
        for (const socket of this.ctx.getWebSockets()) {
          const socketState = socket.deserializeAttachment() as SocketState | null;
          if (!socketState?.ready) continue;
          send(socket, FRAME.row, { seq: outcome.seq, device, batchId }, payload);
        }
      }
      return json({ batchId, seq: outcome.seq, dup: outcome.dup });
    }

    if ((url.pathname === "/tail" || url.pathname === "/diff") && request.method === "PUT") {
      const name = url.pathname === "/tail" ? "sidecar-tail" : "sidecar-diff";
      const body = new Uint8Array(await request.arrayBuffer());
      if (body.byteLength > MAX_SIDECAR_BYTES) return json({ error: "too_large" }, 413);
      this.blobs.put(name, body);
      setMeta(sql, `${name}-type`, request.headers.get("content-type") ?? "application/json");
      return json({ ok: true, bytes: body.byteLength });
    }

    if ((url.pathname === "/tail" || url.pathname === "/diff") && request.method === "GET") {
      const name = url.pathname === "/tail" ? "sidecar-tail" : "sidecar-diff";
      const bytes = this.blobs.get(name);
      if (!bytes) return json({ error: "not_found" }, 404);
      return new Response(bytes, {
        headers: {
          "content-type": getMeta(sql, `${name}-type`) ?? "application/json",
          "content-length": String(bytes.byteLength)
        }
      });
    }

    // ── chat3 transcript plane (docs/chat3-sync.md) ─────────────────────────
    if (url.pathname === "/items" && request.method === "POST") {
      return this.postItems(request, url);
    }

    if (url.pathname === "/items" && request.method === "GET") {
      this.ensureTranscriptTables();
      const frames: Uint8Array[] = [];
      let bodyBytes = 0;
      let truncated = false;
      const add = (frame: Uint8Array): boolean => {
        bodyBytes += 4 + frame.length;
        if (bodyBytes > ITEMS_BODY_CAP) {
          truncated = true;
          return false;
        }
        frames.push(frame);
        return true;
      };
      const itemFrame = (item: { ord: number; idx: number; entry: string; rev: number; bytes: Uint8Array }) =>
        encodeFrame(FRAME.item, { ord: item.ord, idx: item.idx, entry: item.entry, rev: item.rev }, item.bytes);
      // The head is read BEFORE the rows: a write landing mid-response then
      // has a rev above it, so the reader's next feed still fetches it.
      const head = headRev(sql);
      let more = false;
      const sinceRaw = url.searchParams.get("since");
      if (sinceRaw !== null) {
        const since = Number(sinceRaw);
        if (!Number.isInteger(since) || since < 0) return json({ error: "bad_since" }, 400);
        for (const item of itemsSince(sql, since)) {
          if (!add(itemFrame(item))) break;
        }
      } else {
        const beforeRaw = url.searchParams.get("before");
        const before = beforeRaw === null ? undefined : parseItemKey(beforeRaw);
        if (beforeRaw !== null && before === undefined) return json({ error: "bad_before" }, 400);
        const limitRaw = Number(url.searchParams.get("limit") ?? DEFAULT_PAGE_LIMIT);
        const page = itemsPage(sql, before, Number.isFinite(limitRaw) ? limitRaw : DEFAULT_PAGE_LIMIT);
        more = page.more;
        for (const item of page.items) {
          if (!add(itemFrame(item))) break;
        }
      }
      // Open parts ride every read so an HTTP-only viewer (or one that joined
      // mid-stream) sees in-progress text without a socket.
      if (!truncated) {
        for (const part of this.live.values()) {
          for (const frame of part.frames) if (!add(frame)) break;
          if (truncated) break;
        }
      }
      if (!truncated) frames.push(encodeFrame(FRAME.itemsDone, { headRev: head, more }));
      return new Response(lengthPrefixed(frames), {
        headers: { "content-type": "application/octet-stream" }
      });
    }

    if (url.pathname.startsWith("/outputs/")) {
      const part = decodeURIComponent(url.pathname.slice("/outputs/".length));
      if (!PART_RE.test(part)) return json({ error: "bad_part" }, 400);
      this.ensureTranscriptTables();
      if (request.method === "PUT") {
        const declared = Number(request.headers.get("content-length") ?? "0");
        if (declared > MAX_OUTPUT_BYTES) return json({ error: "too_large" }, 413);
        const body = new Uint8Array(await request.arrayBuffer());
        if (body.byteLength > MAX_OUTPUT_BYTES) return json({ error: "too_large" }, 413);
        putOutput(sql, part, body);
        return json({ ok: true, bytes: body.byteLength });
      }
      if (request.method === "GET") {
        const bytes = getOutput(sql, part);
        if (!bytes) return json({ error: "not_found" }, 404);
        return new Response(bytes, {
          headers: {
            "content-type": "text/plain; charset=utf-8",
            "content-length": String(bytes.byteLength),
            // Outputs are written once, when the tool settles.
            "cache-control": "private, max-age=300"
          }
        });
      }
    }

    if (url.pathname === "/stats" && request.method === "GET") {
      this.sweepPresence();
      return json({
        ...logStats(sql),
        connectedSockets: this.ctx.getWebSockets().length,
        presence: Object.fromEntries(this.presence),
        // The ONLY per-device attribution surface — kept from the 2026-08-05
        // incident tooling (SessionRoom's /stats pushOutcomes).
        pushOutcomes: JSON.parse(getMeta(sql, "pushOutcomes") ?? "{}") as Record<
          string,
          PushOutcome
        >,
        lastBackupSeq: Number(getMeta(sql, "backupSeq") ?? "0"),
        // What this DO instance has cost since it last started, in the
        // runtime's own billing units (in memory; resets on eviction).
        billing: { ...this.meter },
        ...(this.hasTranscriptTables()
          ? { transcript: { ...transcriptStats(sql), liveParts: this.live.size, liveBytes: this.liveBytes } }
          : {})
      });
    }

    if (url.pathname === "/reset" && request.method === "POST") {
      // Operator wipe. Recovery is host-driven: the host detects
      // `headSeq < cursor` on its next hello and re-seeds via checkpoint —
      // same shape as the registry reset recipe.
      sql.exec("DELETE FROM rows");
      sql.exec("DELETE FROM meta");
      sql.exec("DELETE FROM blobs");
      sql.exec("DELETE FROM chat_wake");
      if (this.hasTranscriptTables()) resetTranscript(sql);
      this.live.clear();
      this.liveBytes = 0;
      for (const ws of this.ctx.getWebSockets()) {
        try {
          ws.close(4410, "chat room reset");
        } catch {
          /* already gone */
        }
      }
      return json({ ok: true });
    }

    return json({ error: "not found" }, 404);
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
        await this.handlePush(ws, state, frame.header, frame.payload);
        return;
      case FRAME.presence:
        this.handlePresence(ws, state, frame.header, frame.payload);
        return;
      case FRAME.probe:
        send(ws, FRAME.probeOk, { headSeq: headSeq(this.sql) });
        return;
      case FRAME.live:
        this.handleLive(ws, state, frame.header, frame.payload, new Uint8Array(message));
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

  private handleHello(ws: WebSocket, state: SocketState, header: Record<string, unknown>): void {
    if (typeof header.device === "string" && header.device.length > 0) {
      state.device = header.device;
    }
    state.ready = true;
    ws.serializeAttachment(state);
    const sql = this.sql;
    const stats = logStats(sql);
    const frontier = this.blobs.get(FRONTIER_BLOB) ?? new Uint8Array(0);
    // Metadata + frontier only — the CLIENT decides what to load next
    // (frontier included in local doc → rows only; not included → GET
    // /checkpoint first; cursor > headSeq → server lost state, re-seed).
    send(
      ws,
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

  private handleRowsReq(ws: WebSocket, state: SocketState, header: Record<string, unknown>): void {
    if (!state.ready) {
      send(ws, FRAME.error, { code: "hello_first", message: "rows before hello" });
      return;
    }
    const after = typeof header.after === "number" && header.after >= 0 ? header.after : 0;
    const exclude = header.excludeOwn === true ? state.device : undefined;
    const sql = this.sql;
    for (const row of rowsAfter(sql, after, exclude)) {
      send(ws, FRAME.row, { seq: row.seq, device: row.device, batchId: row.batchId }, row.bytes);
    }
    send(ws, FRAME.rowsDone, { headSeq: headSeq(sql) });
  }

  private async handlePush(
    ws: WebSocket,
    state: SocketState,
    header: Record<string, unknown>,
    payload: Uint8Array
  ): Promise<void> {
    const batchId = typeof header.batchId === "string" ? header.batchId : "";
    // Push errors carry the batchId so clients can RETIRE permanently
    // rejected batches from their replay queues (an unretireable batch
    // replays on every reconnect forever — the wedge class this replaces).
    if (!state.ready || batchId === "" || batchId.length > 128) {
      this.recordPush(state.device, false);
      send(ws, FRAME.error, { code: "bad_push", message: "hello first / malformed push", batchId });
      return;
    }
    if (!this.admitQuota(state.device, payload.byteLength)) {
      this.recordPush(state.device, false);
      send(ws, FRAME.error, { code: "quota", message: "per-device push quota exceeded", batchId });
      return;
    }
    const outcome = await this.acceptRow(state.device, batchId, payload, state.userId, state.chatId ?? "", state.hostDevice ?? "");
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
      // Live relay to every OTHER ready socket — the sender has its own
      // bytes; it gets the ack (contrast RegistryRoom, whose LWW merge means
      // the sender must see the merged truth — here bytes are opaque and
      // Loro convergence is the client's business).
      for (const socket of this.ctx.getWebSockets()) {
        if (socket === ws) continue;
        const socketState = socket.deserializeAttachment() as SocketState | null;
        if (!socketState?.ready) continue;
        send(
          socket,
          FRAME.row,
          { seq: outcome.seq, device: state.device, batchId },
          payload
        );
      }
    }
    send(ws, FRAME.ack, { batchId, seq: outcome.seq, dup: outcome.dup });
  }

  // ── chat3 transcript plane ───────────────────────────────────────────────

  /** gen-2 rooms never create the chat3 tables; the first transcript call
   * on an instance does (idempotent, so a cold instance just re-checks). */
  private ensureTranscriptTables(): void {
    if (this.transcriptReady) return;
    ensureTranscript(this.sql);
    this.transcriptReady = true;
  }

  private hasTranscriptTables(): boolean {
    if (this.transcriptReady) return true;
    return [...this.sql.exec("SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = 'items'")].length > 0;
  }

  /** Host batch upsert: a u32-LE length-prefixed run of `itemPut` frames.
   * The JSON reply is the ack (per item: its rev, or why it was refused);
   * changed rows are relayed to every other device's sockets. */
  private async postItems(request: Request, url: URL): Promise<Response> {
    const declared = Number(request.headers.get("content-length") ?? "0");
    if (declared > MAX_ITEMS_BODY_BYTES) return json({ error: "too_large" }, 413);
    const body = new Uint8Array(await request.arrayBuffer());
    if (body.byteLength > MAX_ITEMS_BODY_BYTES) return json({ error: "too_large" }, 413);
    const frames = splitLengthPrefixed(body);
    if (!frames) return json({ error: "bad_body" }, 400);
    this.ensureTranscriptTables();
    const sql = this.sql;
    const device = url.searchParams.get("device") ?? "";
    const results: Record<string, unknown>[] = [];
    const relay: Uint8Array[] = [];
    await this.ctx.storage.transaction(async () => {
      let changed = false;
      for (const raw of frames) {
        const frame = decodeFrame(raw);
        if (!frame || frame.type !== FRAME.itemPut) {
          results.push({ error: "bad_frame" });
          continue;
        }
        const { header } = frame;
        const invalid = validateItemKey(header);
        if (invalid) {
          results.push({ ord: header.ord, idx: header.idx, error: invalid });
          continue;
        }
        const key = { ord: header.ord as number, idx: header.idx as number, entry: header.entry as string };
        const outcome = putItem(sql, key, frame.payload);
        if (!outcome.ok) {
          results.push({ ord: key.ord, idx: key.idx, error: outcome.error });
          continue;
        }
        results.push({ ord: key.ord, idx: key.idx, rev: outcome.rev, changed: outcome.changed });
        if (!outcome.changed) continue;
        changed = true;
        this.dropLive(liveKey(key.ord, key.idx));
        relay.push(encodeFrame(FRAME.item, { ...key, rev: outcome.rev }, frame.payload));
      }
      // One dirty mark per batch, and none while already dirty: a flag that
      // is already "1" would only bill a row to rewrite itself.
      if (changed && getMeta(sql, "backupDirty") !== "1") {
        this.backupDirty();
        await this.scheduleAlarm();
      }
    });
    if (relay.length > 0) {
      for (const socket of this.ctx.getWebSockets()) {
        const socketState = socket.deserializeAttachment() as SocketState | null;
        if (!socketState?.ready || (device !== "" && socketState.device === device)) continue;
        for (const frame of relay) sendRaw(socket, frame);
      }
    }
    return json({ headRev: headRev(sql), results });
  }

  /** Relay an open part to the other sockets and keep enough of it in memory
   * to rebuild it for a late viewer. Never touches storage. */
  private handleLive(
    ws: WebSocket,
    state: SocketState,
    header: Record<string, unknown>,
    payload: Uint8Array,
    raw: Uint8Array
  ): void {
    if (!state.ready) return;
    if (validateItemKey(header) || payload.byteLength > MAX_LIVE_PAYLOAD_BYTES) return;
    if (header.op !== "part" && header.op !== "append") return;
    const key = liveKey(header.ord as number, header.idx as number);
    const frame = raw.slice();
    if (header.op === "part") {
      this.dropLive(key);
      this.live.set(key, { frames: [frame], bytes: frame.byteLength });
      this.liveBytes += frame.byteLength;
    } else {
      // An append with no base in memory can't rebuild anything (the room
      // restarted mid-part); it is still relayed to viewers that have one.
      const part = this.live.get(key);
      if (part) {
        this.live.delete(key); // re-insert: the map's order is recency
        part.frames.push(frame);
        part.bytes += frame.byteLength;
        this.liveBytes += frame.byteLength;
        if (part.bytes > LIVE_PART_MAX_BYTES) this.liveBytes -= part.bytes;
        else this.live.set(key, part);
      }
    }
    while (this.liveBytes > LIVE_TOTAL_MAX_BYTES) {
      const oldest = this.live.keys().next();
      if (oldest.done) break;
      this.dropLive(oldest.value);
    }
    for (const socket of this.ctx.getWebSockets()) {
      if (socket === ws) continue;
      const socketState = socket.deserializeAttachment() as SocketState | null;
      if (!socketState?.ready) continue;
      sendRaw(socket, frame);
    }
  }

  private dropLive(key: string): void {
    const part = this.live.get(key);
    if (!part) return;
    this.live.delete(key);
    this.liveBytes -= part.bytes;
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
    // Broadcast-only relay of the opaque payload — no EphemeralStore, no
    // storage; a device that joins later simply waits for the next beat.
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
    const sql = this.sql;
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

  /** SQL rows, wake receipt, and alarm are one SQLite storage transaction.
   * ACKs therefore cannot outrun the durable host-discovery obligation. */
  private async acceptRow(device: string, batch: string, payload: Uint8Array, user: string, chat: string, host: string) {
    const sql = this.sql;
    const outcome = await this.ctx.storage.transaction(async () => {
      const result = appendRow(sql, device, batch, payload, Date.now());
      if (!result.ok) return result;
      if (!result.dup) this.backupDirty();
      // Host output must not feed a wake loop. Legacy senders without a
      // hint keep their separate nudge path; hints are authenticated routing,
      // never permission to execute (the host still validates its document).
      if (host && host !== device) queueChatWake(sql, chat, host, user);
      await this.scheduleAlarm();
      return result;
    });
    if (outcome.ok && host && host !== device) this.ctx.waitUntil(this.flushWake());
    return outcome;
  }

  private backupDirty(): void {
    const sql = this.sql;
    setMeta(sql, "backupDirty", "1");
    if (!getMeta(sql, "backupDue")) setMeta(sql, "backupDue", String(Date.now() + DAY_MS));
  }

  private async markBackupDirty(): Promise<void> {
    await this.ctx.storage.transaction(async () => {
      this.backupDirty();
      await this.scheduleAlarm();
    });
  }

  /** Always called inside a storage transaction: a backup can't overwrite
   * an earlier wake alarm, including when requests interleave with retries. */
  private async scheduleAlarm(): Promise<void> {
    const sql = this.sql;
    const wake = pendingChatWake(sql);
    const backup = getMeta(sql, "backupDirty") === "1"
      ? Number(getMeta(sql, "backupDue") ?? Date.now() + DAY_MS) : Infinity;
    const next = Math.min(wake?.next_at ?? Infinity, backup);
    if (!Number.isFinite(next)) return;
    const existing = await this.ctx.storage.getAlarm();
    if (existing === null || next < existing) await this.ctx.storage.setAlarm(next);
  }

  private flushWake(): Promise<void> {
    if (this.wakeFlight) return this.wakeFlight;
    this.wakeFlight = this.deliverWake().finally(() => { this.wakeFlight = undefined; });
    return this.wakeFlight;
  }

  private async deliverWake(): Promise<void> {
    const sql = this.sql;
    const wake = pendingChatWake(sql);
    if (!wake) return;
    let accepted = false;
    let rejected = false;
    try {
      const room = this.env.DEVICE_ROOMS.get(this.env.DEVICE_ROOMS.idFromName(`d2/${wake.host_device}`));
      const response = await room.fetch("https://device/nudge", {
        method: "POST", headers: { [AUTH_USER_HEADER]: wake.user_id },
        body: JSON.stringify({ chatId: wake.chat_id }), signal: AbortSignal.timeout(5000)
      });
      accepted = response.ok;
      // Another owner's device (403) or a chat id it refuses (400): no retry
      // can succeed. Unclaimed (404) and queue-full (503) retry.
      rejected = PERMANENT_WAKE_REJECTIONS.has(response.status);
      await response.arrayBuffer();
    } catch {
      // A failed forward remains owned by this room, even with no viewer.
    }
    const exhausted = !accepted && wake.attempts + 1 >= MAX_WAKE_ATTEMPTS;
    if (rejected || exhausted) {
      console.warn("chat2 host wake dropped", {
        chat: wake.chat_id, host: wake.host_device, rejected, attempts: wake.attempts + 1
      });
    }
    await this.ctx.storage.transaction(async () => {
      if (accepted || rejected || exhausted) finishChatWake(sql, wake.token);
      else retryChatWake(sql, wake);
      await this.scheduleAlarm();
    });
  }

  /** Wake retry + nightly backup share the DO's single durable alarm. */
  async alarm(): Promise<void> {
    try {
      const wake = pendingChatWake(this.sql);
      if (wake && wake.next_at <= Date.now()) await this.flushWake();
      const due = Number(getMeta(this.sql, "backupDue") ?? "0");
      if (due <= Date.now()) {
        try { await this.backup(); }
        catch (error) {
          // Do not exhaust the runtime's finite automatic retries or spin
          // on an expired deadline during an R2 outage.
          setMeta(this.sql, "backupDue", String(Date.now() + 60_000));
          console.warn("chat2 backup retrying", error);
        }
      }
    } finally {
      await this.ctx.storage.transaction(async () => { await this.scheduleAlarm(); });
    }
  }

  /** Seq-monotonic backup; concurrent new rows retain their dirty flag. */
  private async backup(): Promise<void> {
    const sql = this.sql;
    if (getMeta(sql, "backupDirty") !== "1") return; // idle: stop the chain
    const head = headSeq(sql);
    // chat3 rooms also back up their transcript rows (tool outputs are not:
    // they are re-derivable on the host and dwarf everything else).
    const transcript = this.hasTranscriptTables();
    const rev = transcript ? headRev(sql) : 0;
    if (head > Number(getMeta(sql, "backupSeq") ?? "0") || rev > Number(getMeta(sql, "backupRev") ?? "0")) {
      const rows = [...rowsAfter(sql, 0)].map((row) => ({
        seq: row.seq,
        device: row.device,
        batchId: row.batchId,
        bytes: encodeBase64(row.bytes)
      }));
      const items = transcript
        ? [...itemsSince(sql, 0)].map((item) => ({
            ord: item.ord,
            idx: item.idx,
            entry: item.entry,
            rev: item.rev,
            bytes: encodeBase64(item.bytes)
          }))
        : undefined;
      const checkpoint = this.blobs.get(CHECKPOINT_BLOB);
      const frontier = this.blobs.get(FRONTIER_BLOB);
      await this.env.BLOBS.put(
        `backup/chat2/${this.ctx.id.toString()}/latest.json`,
        JSON.stringify({
          at: Date.now(),
          ...logStats(sql),
          checkpoint: checkpoint ? encodeBase64(checkpoint) : null,
          frontier: frontier ? encodeBase64(frontier) : null,
          rows,
          ...(items ? { items } : {})
        })
      );
      setMeta(sql, "backupSeq", String(head));
      if (transcript) setMeta(sql, "backupRev", String(rev));
    }
    if (headSeq(sql) === head && (!transcript || headRev(sql) === rev)) {
      setMeta(sql, "backupDirty", "0");
      sql.exec("DELETE FROM meta WHERE key='backupDue'");
    } else {
      // Rows landed during the upload: they stay dirty for the NEXT nightly
      // backup. Leaving the passed deadline re-armed the alarm at once and
      // re-uploaded the whole log back to back while the chat streamed.
      setMeta(sql, "backupDue", String(Date.now() + DAY_MS));
    }
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

const sendRaw = (ws: WebSocket, frame: Uint8Array): void => {
  try {
    ws.send(frame.buffer.slice(frame.byteOffset, frame.byteOffset + frame.byteLength) as ArrayBuffer);
  } catch {
    /* socket already gone; hibernation API cleans it up */
  }
};

const liveKey = (ord: number, idx: number): string => `${ord}:${idx}`;

/** u32-LE length-prefixed frames — the `/rows` body encoding. */
const lengthPrefixed = (frames: Uint8Array[]): Uint8Array => {
  const total = frames.reduce((n, f) => n + 4 + f.length, 0);
  const body = new Uint8Array(total);
  const view = new DataView(body.buffer);
  let off = 0;
  for (const f of frames) {
    view.setUint32(off, f.length, true);
    body.set(f, off + 4);
    off += 4 + f.length;
  }
  return body;
};

/** Inverse of `lengthPrefixed`; undefined if a length runs past the body. */
const splitLengthPrefixed = (body: Uint8Array): Uint8Array[] | undefined => {
  const view = new DataView(body.buffer, body.byteOffset, body.byteLength);
  const frames: Uint8Array[] = [];
  let off = 0;
  while (off < body.byteLength) {
    if (off + 4 > body.byteLength) return undefined;
    const len = view.getUint32(off, true);
    if (off + 4 + len > body.byteLength) return undefined;
    frames.push(body.subarray(off + 4, off + 4 + len));
    off += 4 + len;
  }
  return frames;
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

const encodeBase64 = (bytes: Uint8Array): string => {
  let bin = "";
  for (let i = 0; i < bytes.length; i += 0x8000) {
    bin += String.fromCharCode(...bytes.subarray(i, i + 0x8000));
  }
  return btoa(bin);
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
