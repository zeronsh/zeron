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
import { readCapped } from "./read-capped";
import {
  appendRow,
  CHECKPOINT_BLOB,
  commitCheckpoint,
  commitStagedCheckpoint,
  ensureChatLog,
  flushHeadSeq,
  FRONTIER_BLOB,
  getMeta,
  headSeq,
  logStats,
  MAX_ROW_BYTES,
  rowsAfter,
  seqFloor,
  setMeta,
  STAGED_CHECKPOINT_BLOB
} from "./chat-log";
import { decodeFrame, encodeFrame, FRAME, RELAY_FRAME_TYPES } from "./chat-frames";
import {
  decodeVersion, drainQueueIntoRows, enqueueRow, ensureRelayQueue, hasRelayQueue, markDelivered,
  markDeliveredByOthers, queueHolds, queueLength, queueRows, versionIncludes
} from "./chat-relay";
import { AUTH_ORG_HEADER, AUTH_USER_HEADER, type Env } from "./env";
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
/** A checkpoint past that comes in parts (`/checkpoint-part`, then
 * `/checkpoint?staged=<size>`), each at most this: a long chat's state alone
 * outgrows one request (2026-11-07: 35 and 39 MB, 413 on every post), and a
 * part is all the room holds in memory at once. */
export const MAX_CHECKPOINT_PART_BYTES = 8 * 1024 * 1024;
/** The most a checkpoint sent in parts may come to. */
export const MAX_STAGED_CHECKPOINT_BYTES = 256 * 1024 * 1024;
/** A checkpoint past this goes to the nightly backup as an object of its
 * own, streamed, not inline in its JSON as base64. */
const BACKUP_INLINE_CHECKPOINT_BYTES = 8 * 1024 * 1024;
/** Presence beats older than this are swept before relay/stats. */
const PRESENCE_TTL_MS = 30_000;
/** Per-device push quota, rolling window (in-memory; resets on hibernation —
 * it exists to contain a runaway client loop, not to meter honest traffic). */
const QUOTA_WINDOW_MS = 60_000;
const QUOTA_MAX_PUSHES = 300;
const QUOTA_MAX_BYTES = 8 * 1024 * 1024;
/** How long per-push bookkeeping (the stored head copy, the per-device push
 * counts) may lag. Writing them per push made up 2 of a push's 5 billed rows;
 * an active room now pays a few rows every five minutes instead. */
const BOOKKEEPING_FLUSH_MS = 5 * 60_000;
/** Relay frames aren't stored and heal if dropped, so past this budget the DO
 * drops them instead of refusing them. It exists to contain a runaway client;
 * a host streaming every 120 ms stays far under it. */
const RELAY_QUOTA_MAX_FRAMES = 6000;
const RELAY_QUOTA_MAX_BYTES = 256 * 1024 * 1024;
/** How often an HTTP reader without relay frames re-sends `legacyPeer`, and how
 * long after its last read it still counts as here. */
const LEGACY_READ_SIGNAL_MS = 30_000;
const LEGACY_READ_TTL_MS = 2 * 60_000;
/** Header budget: about 100 batch ids per `delivered`. */
const MAX_DELIVERED_IDS = 100;
/** How often an arriving reader may wake a host that isn't in the room. */
const HOST_WAKE_MIN_MS = 60_000;
const RELAY_DIRTY = "relayDirty";

interface SocketState {
  userId: string;
  device: string;
  /** Set once a valid hello established the session. */
  ready?: boolean;
  chatId?: string;
  hostDevice?: string;
  /** This socket's hello asked for the relay layer, and the layer is on. */
  relay?: boolean;
  /** The caller's org, to find the chat's registry. Only with the layer on. */
  orgId?: string;
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
  /** device → last presence beat (epoch ms). Memory-only by construction. */
  private readonly presence = new Map<string, number>();
  /** device → rolling push quota window. Memory-only. */
  private readonly quotas = new Map<string, QuotaWindow>();
  /** device → push counts not yet in `meta.pushOutcomes`. /stats merges them.
   * Eviction can lose the counts since the last flush; they're diagnostics. */
  private readonly unflushedOutcomes = new Map<string, PushOutcome>();
  /** When the bookkeeping flush is due; undefined = nothing to flush. The alarm
   * it arms is durable, so losing this field to eviction loses no flush. */
  private flushAt?: number;
  private wakeFlight?: Promise<void>;
  /** device → rolling relay budget. Memory-only. */
  private readonly relayQuotas = new Map<string, QuotaWindow>();
  private relayQueueReady = false;
  /** Last read of `/rows` by a client that can't take relay frames. */
  private legacyReadAt = 0;
  private legacySignaledAt = 0;
  private hostWokenAt = 0;
  /** `relayDirty` is known to be set: skip even the read. */
  private relayDirtyMarked = false;
  /** Layer off: whether to look for a queue left from when it was on. */
  private staleQueueChecked = false;

  constructor(ctx: DurableObjectState, env: Env) {
    this.ctx = ctx;
    this.env = env;
    ensureChatLog(ctx.storage.sql);
    ensureChatWakes(ctx.storage.sql);
    this.blobs = createBlobStore(ctx.storage.sql);
    // Runtime-answered keepalive; proves nothing about this DO's health.
    // Clients judge liveness by probe frames (same caveat as RegistryRoom).
    ctx.setWebSocketAutoResponse(new WebSocketRequestResponsePair("ping", "pong"));
  }

  // ── HTTP surface (only reachable through the authed Worker) ──────────────

  async fetch(request: Request): Promise<Response> {
    const url = new URL(request.url);
    const userId = request.headers.get(AUTH_USER_HEADER);
    if (!userId) return json({ error: "unauthenticated" }, 401);

    const sql = this.ctx.storage.sql;
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
      const orgId = request.headers.get(AUTH_ORG_HEADER);
      if (orgId) state.orgId = orgId;
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
      // A relay-era checkpoint prunes nothing, whatever floor the host last saw.
      const relayEra = url.searchParams.get("atFloor") === "1";
      const atFloor = this.relayEnabled() && relayEra;
      // A relay-era checkpoint can be all that's stored, so while the layer is on no
      // checkpoint may replace the room's with less: an ordinary one from a viewer or
      // an older build is behind as often as not. Asked before any of it is read.
      if ((relayEra || this.relayEnabled()) && !this.checkpointMovesForward(frontier)) {
        return json({ error: "stale" }, 409);
      }
      const covered = atFloor ? seqFloor(sql) : seqCovered;
      // Sent in parts: the staged parts become it, as they are.
      const staged = url.searchParams.get("staged");
      let outcome: ReturnType<typeof commitCheckpoint>;
      if (staged !== null) {
        const size = Number(staged);
        if (!Number.isSafeInteger(size) || size <= 0) return json({ error: "bad_staged" }, 400);
        outcome = commitStagedCheckpoint(sql, this.blobs, covered, frontier, size, Date.now());
      } else {
        // (Refused before it's read: an engine from before parts re-posted a
        // long chat's whole snapshot, 35 MB and more, every few seconds.)
        const body = await readCapped(request, MAX_CHECKPOINT_BYTES);
        if (!body) return json({ error: "too_large" }, 413);
        outcome = commitCheckpoint(sql, this.blobs, covered, frontier, body, Date.now());
      }
      if (!outcome.ok) return json({ error: outcome.error }, 409);
      await this.markBackupDirty();
      // A heal covers everything relayed so far: what the queue held back can be
      // stored now, without leaving a row that no checkpoint can fill.
      if (relayEra && url.searchParams.get("heal") === "1") this.healed();
      else if (atFloor) this.resyncLegacySockets();
      return json({ ok: true, seqFloor: outcome.seqFloor, pruned: outcome.pruned });
    }

    // Claim-on-first-contact for the HTTP surface too — same client-minted-id
    // discipline as /ws. The /rows twins predate the pull-first HTTPS
    // transport and 404'd an unclaimed room, which deadlocked a brand-new
    // chat on WS-hostile networks: the sender's push 404s, the host's pull
    // 404s, and the only claimants (WS join, checkpoint heal) never run.
    if (!owner) setMeta(sql, "owner", userId);
    else if (owner !== userId) return json({ error: "forbidden" }, 403);

    if (url.pathname === "/checkpoint-part" && request.method === "POST") {
      // One piece of a checkpoint too large for one request, at `offset` in
      // it: appended to the room's staged checkpoint (offset 0 starts it
      // again), which `/checkpoint?staged=<size>` then commits.
      const offset = Number(url.searchParams.get("offset") ?? "");
      if (!Number.isSafeInteger(offset) || offset < 0) return json({ error: "bad_offset" }, 400);
      const body = await readCapped(request, MAX_CHECKPOINT_PART_BYTES);
      if (!body) return json({ error: "too_large" }, 413);
      if (body.byteLength === 0) return json({ error: "empty" }, 400);
      if (offset === 0) this.blobs.delete(STAGED_CHECKPOINT_BLOB);
      const staged = this.blobs.size(STAGED_CHECKPOINT_BLOB) ?? 0;
      if (offset !== staged) return json({ error: "offset", staged }, 409);
      if (staged + body.byteLength > MAX_STAGED_CHECKPOINT_BYTES) return json({ error: "too_large" }, 413);
      this.blobs.append(STAGED_CHECKPOINT_BLOB, body);
      return json({ ok: true, staged: staged + body.byteLength });
    }

    if (url.pathname === "/checkpoint" && request.method === "GET") {
      // Read a chunk row at a time: a checkpoint sent in parts can be more
      // than the room holds in memory.
      const size = this.blobs.size(CHECKPOINT_BLOB);
      if (size === undefined) return json({ error: "not_found" }, 404);
      // Range-resumable (bytes=N- only): a 1MB load over a 1.2Mbps link that
      // dies at byte 800k resumes, where s2's export-per-join restarted.
      const range = parseRangeStart(request.headers.get("range"));
      if (range !== null && range >= size) {
        return new Response(null, {
          status: 416,
          headers: { "content-range": `bytes */${size}` }
        });
      }
      const headers = new Headers({
        "content-type": "application/octet-stream",
        "content-length": String(size - (range ?? 0)),
        "accept-ranges": "bytes",
        "x-chat2-checkpoint-seq": getMeta(sql, "checkpointSeq") ?? "0"
      });
      if (range !== null) {
        headers.set("content-range", `bytes ${range}-${size - 1}/${size}`);
      }
      const body = this.blobs.stream(CHECKPOINT_BLOB, range ?? 0).pipeThrough(new FixedLengthStream(size - (range ?? 0)));
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
      if (url.searchParams.get("relay") !== "1" || url.searchParams.get("fallback") === "1") {
        this.noteLegacyRead(device);
        const orgId = request.headers.get(AUTH_ORG_HEADER);
        if (orgId && this.relayEnabled() && this.relayDirty()) {
          this.wakeAbsentHost({ userId, device, chatId: url.searchParams.get("chatId") ?? "", orgId });
        }
      } else {
        this.drainStaleQueue();
      }
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

    if (url.pathname === "/stats" && request.method === "GET") {
      this.sweepPresence();
      return json({
        ...logStats(sql),
        ...(this.relayEnabled()
          ? { relayQueue: queueLength(this.relaySql()), relayDirty: this.relayDirty() }
          : {}),
        connectedSockets: this.ctx.getWebSockets().length,
        presence: Object.fromEntries(this.presence),
        // The ONLY per-device attribution surface — kept from the 2026-08-05
        // incident tooling (SessionRoom's /stats pushOutcomes).
        pushOutcomes: this.mergedOutcomes(),
        lastBackupSeq: Number(getMeta(sql, "backupSeq") ?? "0")
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
      sql.exec("DROP TABLE IF EXISTS relay_queue");
      this.relayQueueReady = false;
      this.unflushedOutcomes.clear();
      this.flushAt = undefined;
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
    if (frame && state.relay && !this.relayEnabled()) {
      // The layer was turned off under a live session, and a socket that survived
      // the deploy still thinks it's on. Its next hello gets the room's real answer,
      // so its host goes back to stored rows now, not when this socket happens to end.
      ws.close(1012, "relay layer off");
      return;
    }
    if (!frame || (RELAY_FRAME_TYPES.has(frame.type) && !this.relayEnabled())) {
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
        send(ws, FRAME.probeOk, { headSeq: headSeq(this.ctx.storage.sql) });
        return;
      case FRAME.relay:
      case FRAME.announce:
      case FRAME.syncReq:
      case FRAME.syncData:
        this.handleRelayFrame(ws, state, frame.type, frame.header, frame.payload);
        return;
      case FRAME.enqueue:
        await this.handleEnqueue(ws, state, frame.header, frame.payload);
        return;
      case FRAME.queueReq:
        this.handleQueueReq(ws, state);
        return;
      case FRAME.delivered:
        await this.handleDelivered(state, frame.header);
        return;
      default:
        send(ws, FRAME.error, { code: "bad_frame", message: `unexpected type ${frame.type}` });
    }
  }

  async webSocketClose(): Promise<void> {
    /* nothing to save: rows are written synchronously on push */
  }

  async webSocketError(): Promise<void> {
    /* ditto */
  }

  private handleHello(ws: WebSocket, state: SocketState, header: Record<string, unknown>): void {
    if (typeof header.device === "string" && header.device.length > 0) {
      state.device = header.device;
    }
    state.ready = true;
    const layer = this.relayEnabled();
    state.relay = layer && header.relay === 1;
    if (!state.relay) delete state.relay;
    ws.serializeAttachment(state);
    if (state.relay && header.relaying === 1) this.markRelayDirty();
    if (layer && !state.relay) {
      this.drainQueueForLegacy();
      this.signalLegacyPeer(state.device);
    } else if (!layer) {
      this.drainStaleQueue();
    }
    if (!state.relay) this.drainForLegacyHost(state);
    // Only for a chat in the relay era: a stored chat's readers need no host.
    if (layer && (header.gen === 3 || this.relayDirty())) this.wakeAbsentHost(state);
    const sql = this.ctx.storage.sql;
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
        rowBytes: stats.rowBytes,
        ...(state.relay
          ? {
              relay: 1,
              legacyPeers: this.legacyPeerCount(),
              ...(this.relayDirty() ? { relayDirty: 1 } : {})
            }
          : {})
      },
      frontier
    );
  }

  // ── relay layer (docs/design/relay-layer.md) ─────────────────────────────

  /** `CHAT_RELAY_LAYER`: "1" for everyone, or the user ids whose rooms offer it. */
  private relayEnabled(): boolean {
    const flag = this.env.CHAT_RELAY_LAYER ?? "";
    if (flag === "" || flag === "1") return flag === "1";
    const owner = getMeta(this.ctx.storage.sql, "owner");
    return !!owner && flag.split(",").some((id) => id.trim() === owner);
  }

  /** The room's SQL, with the queue table in place. */
  private relaySql(): SqlStorage {
    const sql = this.ctx.storage.sql;
    if (!this.relayQueueReady) {
      ensureRelayQueue(sql);
      this.relayQueueReady = true;
    }
    return sql;
  }

  private relaySockets(except?: WebSocket): Array<{ socket: WebSocket; device: string }> {
    const sockets: Array<{ socket: WebSocket; device: string }> = [];
    for (const socket of this.ctx.getWebSockets()) {
      if (socket === except) continue;
      const peer = socket.deserializeAttachment() as SocketState | null;
      if (peer?.ready && peer.relay) sockets.push({ socket, device: peer.device });
    }
    return sockets;
  }

  private legacyPeerCount(): number {
    let count = Date.now() - this.legacyReadAt < LEGACY_READ_TTL_MS ? 1 : 0;
    for (const socket of this.ctx.getWebSockets()) {
      const peer = socket.deserializeAttachment() as SocketState | null;
      if (peer?.ready && !peer.relay) count += 1;
    }
    return count;
  }

  private signalLegacyPeer(device: string): void {
    for (const { socket } of this.relaySockets()) send(socket, FRAME.legacyPeer, { device });
  }

  /** Someone's here and the host isn't. An idle host has closed its socket, so
   * it won't hear them: nobody else can bring them up to date, or heal for an
   * old client. Rate-limited, because a truly offline host stays away. */
  private wakeAbsentHost(state: SocketState): void {
    const hint = state.hostDevice ?? "";
    if (hint) {
      this.wakeIfAbsent(state, hint);
      return;
    }
    // Shipped phones send no hint. The registry knows, and this only runs for a
    // chat in the relay era, where nobody else can bring that reader up to date.
    if (state.relay || !state.orgId) return;
    this.ctx.waitUntil(
      this.chatHost(state).then(host => {
        if (host) this.wakeIfAbsent(state, host);
      })
    );
  }

  private wakeIfAbsent(state: SocketState, host: string): void {
    const chat = state.chatId ?? "";
    if (host === state.device || !validWakeRoute(chat, host)) return;
    const now = Date.now();
    if (now - this.hostWokenAt < HOST_WAKE_MIN_MS) return;
    for (const socket of this.ctx.getWebSockets()) {
      const peer = socket.deserializeAttachment() as SocketState | null;
      if (peer?.ready && peer.device === host) return;
    }
    this.hostWokenAt = now;
    const sql = this.ctx.storage.sql;
    this.ctx.waitUntil(
      this.ctx.storage
        .transaction(async () => {
          queueChatWake(sql, chat, host, state.userId);
          await this.scheduleAlarm();
        })
        .then(() => this.flushWake())
    );
  }

  /** Ops may have been relayed that no checkpoint holds: a stored row can depend
   * on them, and a reader without the layer has no way to fill that gap. It would
   * redial and download the checkpoint forever. Set by a relaying host, cleared by
   * its heal. */
  private relayDirty(): boolean {
    return getMeta(this.ctx.storage.sql, RELAY_DIRTY) === "1";
  }

  private markRelayDirty(): void {
    if (this.relayDirtyMarked) return;
    this.relayDirtyMarked = true;
    if (!this.relayDirty()) setMeta(this.ctx.storage.sql, RELAY_DIRTY, "1");
  }

  /** The host has stored a checkpoint exported after it stopped relaying. */
  private healed(): void {
    this.relayDirtyMarked = false;
    this.ctx.storage.sql.exec("DELETE FROM meta WHERE key = ?", RELAY_DIRTY);
    if (this.relayEnabled()) {
      if (this.legacyPeerCount() > 0) this.drainQueueForLegacy();
    } else {
      this.staleQueueChecked = false;
      this.drainStaleQueue();
    }
    this.resyncLegacySockets();
  }

  /** A relay-era checkpoint may not replace the room's copy with less. An empty
   * doc never can; versions are compared where both can be read. */
  private checkpointMovesForward(frontier: Uint8Array): boolean {
    const held = this.blobs.get(FRONTIER_BLOB);
    if (!held || held.byteLength === 0) return true;
    const next = decodeVersion(frontier);
    if (frontier.byteLength === 0 || next?.size === 0) return false;
    const previous = decodeVersion(held);
    return !next || !previous || versionIncludes(next, previous);
  }

  /** A reader that can't use the queue is here: what's queued goes where it can
   * read it, and where a host on an older build can too. Not while ops are out
   * there that no checkpoint holds: the row would park on that reader for good.
   * It waits for the host's heal, which the reader's arrival has asked for. */
  private drainQueueForLegacy(): void {
    if (this.relayDirty()) return;
    const sql = this.relaySql();
    this.storeDrained(drainQueueIntoRows(sql, Date.now()));
  }

  /** The chat's host is back on a build without the layer. It can't heal, so the
   * drains above would wait for good, and what's queued is messages sent to it. It
   * holds every op those messages depend on, so it can always read them as rows. An
   * old reader may still park on them until a heal; it would have anyway. */
  private drainForLegacyHost(state: SocketState): void {
    if (!this.relayDirty() || !state.orgId) return;
    const sql = this.ctx.storage.sql;
    if (!hasRelayQueue(sql)) return;
    this.ctx.waitUntil(
      this.chatHost(state).then(host => {
        if (host !== state.device || !hasRelayQueue(sql)) return;
        this.storeDrained(drainQueueIntoRows(sql, Date.now()));
        if (!this.relayEnabled()) {
          sql.exec("DROP TABLE relay_queue");
          this.relayQueueReady = false;
        }
      })
    );
  }

  /** The layer was turned off with rows still queued: nothing can read the queue
   * now, and those senders have retired their outboxes. Costs one schema read per
   * object lifetime. A room that never had the layer does nothing else. Waits for
   * the heal, like the drain above. */
  private drainStaleQueue(): void {
    if (this.relayEnabled() || this.staleQueueChecked) return;
    this.staleQueueChecked = true;
    const sql = this.ctx.storage.sql;
    if (!hasRelayQueue(sql) || this.relayDirty()) return;
    this.storeDrained(drainQueueIntoRows(sql, Date.now()));
    sql.exec("DROP TABLE relay_queue");
    this.relayQueueReady = false;
  }

  private storeDrained(rows: ReturnType<typeof drainQueueIntoRows>): void {
    if (rows.length === 0) return;
    // Rollback gate: a release that reads the stored head must find it current.
    if (this.env.CHAT_ROOM_WRITE_THROUGH === "1") flushHeadSeq(this.ctx.storage.sql);
    this.flushAt ??= Date.now() + BOOKKEEPING_FLUSH_MS;
    this.ctx.waitUntil(this.markBackupDirty());
    for (const socket of this.ctx.getWebSockets()) {
      const peer = socket.deserializeAttachment() as SocketState | null;
      if (!peer?.ready) continue;
      for (const row of rows) {
        send(socket, FRAME.row, { seq: row.seq, device: row.device, batchId: row.batchId }, row.bytes);
      }
    }
  }

  /** Old clients only look at the checkpoint when they join, and an idle chat may
   * send no row to make them look. A dropped socket makes them join again. */
  private resyncLegacySockets(): void {
    for (const socket of this.ctx.getWebSockets()) {
      const peer = socket.deserializeAttachment() as SocketState | null;
      if (peer?.ready && !peer.relay) {
        try {
          socket.close(1012, "checkpoint updated");
        } catch {
          /* already gone */
        }
      }
    }
  }

  private noteLegacyRead(device: string): void {
    if (!this.relayEnabled()) {
      this.drainStaleQueue();
      return;
    }
    const now = Date.now();
    this.legacyReadAt = now;
    this.drainQueueForLegacy();
    if (now - this.legacySignaledAt < LEGACY_READ_SIGNAL_MS) return;
    this.legacySignaledAt = now;
    this.signalLegacyPeer(device);
  }

  /** Forward without storing. Anything dropped here heals by version exchange. */
  private handleRelayFrame(
    ws: WebSocket,
    state: SocketState,
    type: typeof FRAME.relay | typeof FRAME.announce | typeof FRAME.syncReq | typeof FRAME.syncData,
    header: Record<string, unknown>,
    payload: Uint8Array
  ): void {
    if (!state.ready || !state.relay) {
      send(ws, FRAME.error, { code: "hello_first", message: "relay before a relay hello" });
      return;
    }
    if (!this.admitRelay(state.device, payload.byteLength)) return;
    if (type === FRAME.relay) this.markRelayDirty();
    const { to, ...rest } = header;
    const forwarded = { ...rest, device: state.device };
    if (type === FRAME.syncReq || type === FRAME.syncData) {
      if (typeof to !== "string") return;
      let delivered = false;
      for (const peer of this.relaySockets(ws)) {
        if (peer.device === to) {
          send(peer.socket, type, forwarded, payload);
          delivered = true;
        }
      }
      // A reader asking a host that has already left: bring it back. Its announce
      // on joining restarts the reader's request.
      if (!delivered && type === FRAME.syncReq && to === state.hostDevice) this.wakeIfAbsent(state, to);
      return;
    }
    for (const peer of this.relaySockets(ws)) send(peer.socket, type, forwarded, payload);
    // A host's announce follows its commits. Telling it each time an old reader
    // is still here keeps it storing for as long as that reader stays.
    if (type === FRAME.announce && this.legacyPeerCount() > 0) {
      send(ws, FRAME.legacyPeer, { device: "" });
    }
  }

  private admitRelay(device: string, bytes: number): boolean {
    const now = Date.now();
    let window = this.relayQuotas.get(device);
    if (!window || now - window.since > QUOTA_WINDOW_MS) {
      window = { since: now, pushes: 0, bytes: 0 };
      this.relayQuotas.set(device, window);
    }
    window.pushes += 1;
    window.bytes += bytes;
    return window.pushes <= RELAY_QUOTA_MAX_FRAMES && window.bytes <= RELAY_QUOTA_MAX_BYTES;
  }

  private async handleEnqueue(
    ws: WebSocket,
    state: SocketState,
    header: Record<string, unknown>,
    payload: Uint8Array
  ): Promise<void> {
    const batchId = typeof header.batchId === "string" ? header.batchId : "";
    if (!state.ready || !state.relay || batchId === "" || batchId.length > 128) {
      this.recordPush(state.device, false);
      send(ws, FRAME.error, { code: "bad_push", message: "relay hello first / malformed enqueue", batchId });
      return;
    }
    if (!this.admitQuota(state.device, payload.byteLength)) {
      this.recordPush(state.device, false);
      send(ws, FRAME.error, { code: "quota", message: "per-device push quota exceeded", batchId });
      return;
    }
    // Live peers get it before the write. Anything sent once a write has begun waits
    // until the write is durable, which put that wait between every send and its
    // host. The copy is only a hint: what the sender is told, and what a host that
    // isn't here reads, still waits for the queue. A write that never lands leaves
    // a host holding a message whose sender retries it, and importing it twice is a
    // no-op. Only a row this write could accept goes early.
    const sql = this.ctx.storage.sql;
    const early =
      payload.byteLength > 0 &&
      payload.byteLength <= MAX_ROW_BYTES &&
      !((this.relayQueueReady || hasRelayQueue(sql)) && queueHolds(sql, batchId));
    if (early) {
      for (const peer of this.relaySockets(ws)) {
        send(peer.socket, FRAME.relay, { device: state.device, queued: true, batchId }, payload);
      }
    }
    this.relaySql();
    const host = state.hostDevice ?? "";
    const chat = state.chatId ?? "";
    // The row and its wake commit together, as a stored row and its wake do:
    // once `queued` goes out, the host is owed a delivery.
    const outcome = await this.ctx.storage.transaction(async () => {
      const result = enqueueRow(sql, state.device, batchId, payload, Date.now());
      if (!result.ok) return result;
      if (host && host !== state.device) queueChatWake(sql, chat, host, state.userId);
      this.flushAt ??= Date.now() + BOOKKEEPING_FLUSH_MS;
      await this.scheduleAlarm();
      return result;
    });
    if (!outcome.ok) {
      this.recordPush(state.device, false);
      send(ws, FRAME.error, { code: outcome.error, message: `push rejected: ${outcome.error}`, batchId });
      return;
    }
    if (host && host !== state.device) this.ctx.waitUntil(this.flushWake());
    this.recordPush(state.device, true);
    if (!outcome.dup) {
      if (!early) {
        for (const peer of this.relaySockets(ws)) {
          send(peer.socket, FRAME.relay, { device: state.device, queued: true, batchId }, payload);
        }
      }
      // A socket without the layer never hears relay frames: it reads rows.
      if (this.hasLegacySocket()) this.drainQueueForLegacy();
    }
    send(ws, FRAME.queued, { batchId, dup: outcome.dup });
  }

  private handleQueueReq(ws: WebSocket, state: SocketState): void {
    if (!state.ready || !state.relay) {
      send(ws, FRAME.error, { code: "hello_first", message: "queue before a relay hello" });
      return;
    }
    let count = 0;
    for (const row of queueRows(this.relaySql())) {
      send(ws, FRAME.relay, { device: row.device, queued: true, batchId: row.batchId }, row.bytes);
      count += 1;
    }
    send(ws, FRAME.queueDone, { count });
  }

  private hasLegacySocket(): boolean {
    return this.ctx.getWebSockets().some(socket => {
      const peer = socket.deserializeAttachment() as SocketState | null;
      return peer?.ready === true && !peer.relay;
    });
  }

  /** Only the chat's host may retire a queue row: anyone else who has saved it
   * may be gone when the host needs it. The registry is the authority, not a
   * device's own idea of who hosts. No answer keeps the row: it's redelivered. */
  private async handleDelivered(state: SocketState, header: Record<string, unknown>): Promise<void> {
    if (!state.ready || !state.relay || !Array.isArray(header.batchIds)) return;
    const ids = header.batchIds
      .filter((id): id is string => typeof id === "string")
      .slice(0, MAX_DELIVERED_IDS);
    if (!state.orgId) {
      markDeliveredByOthers(this.relaySql(), state.device, ids);
      return;
    }
    const host = await this.chatHost(state);
    if (host === undefined) return;
    if (host === null) {
      markDeliveredByOthers(this.relaySql(), state.device, ids);
    } else if (host === state.device) {
      markDelivered(this.relaySql(), ids);
    } else {
      console.warn("chat relay: delivered from a device that doesn't host this chat", state.device);
    }
  }

  /** The registry's answer: a device, null if it has no such chat row yet, or
   * undefined if it couldn't be asked. */
  private async chatHost(state: SocketState): Promise<string | null | undefined> {
    try {
      const room = this.env.REGISTRY_ROOMS.get(
        this.env.REGISTRY_ROOMS.idFromName(`reg1/${state.orgId}/${state.userId}`)
      );
      const reply = await room.fetch(`https://registry/chat-host?chat=${encodeURIComponent(state.chatId ?? "")}`, {
        headers: { [AUTH_USER_HEADER]: state.userId }
      });
      if (!reply.ok) return undefined;
      const { deviceId } = (await reply.json()) as { deviceId?: string | null };
      return deviceId ?? null;
    } catch (error) {
      console.warn("chat relay: registry unavailable", String(error));
      return undefined;
    }
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
    const key = device === "" ? "(unknown)" : device;
    const entry = this.unflushedOutcomes.get(key) ?? { ok: 0, rejected: 0, lastOkAt: 0 };
    if (ok) {
      entry.ok += 1;
      entry.lastOkAt = Date.now();
    } else {
      entry.rejected += 1;
    }
    this.unflushedOutcomes.set(key, entry);
    // An accepted row already armed the flush in its own transaction.
    if (!ok && this.flushAt === undefined) {
      this.flushAt = Date.now() + BOOKKEEPING_FLUSH_MS;
      this.ctx.waitUntil(this.ctx.storage.transaction(async () => { await this.scheduleAlarm(); }));
    }
  }

  /** Stored push counts plus the ones still in memory. */
  private mergedOutcomes(): Record<string, PushOutcome> {
    const outcomes = JSON.parse(getMeta(this.ctx.storage.sql, "pushOutcomes") ?? "{}") as Record<
      string,
      PushOutcome
    >;
    for (const [key, delta] of this.unflushedOutcomes) {
      const entry = outcomes[key] ?? { ok: 0, rejected: 0, lastOkAt: 0 };
      entry.ok += delta.ok;
      entry.rejected += delta.rejected;
      entry.lastOkAt = Math.max(entry.lastOkAt, delta.lastOkAt);
      outcomes[key] = entry;
    }
    return outcomes;
  }

  private flushBookkeeping(): void {
    const sql = this.ctx.storage.sql;
    flushHeadSeq(sql);
    if (this.unflushedOutcomes.size > 0) {
      setMeta(sql, "pushOutcomes", JSON.stringify(this.mergedOutcomes()));
      this.unflushedOutcomes.clear();
    }
    this.flushAt = undefined;
  }

  /** SQL rows, wake receipt, and alarm are one SQLite storage transaction.
   * ACKs therefore cannot outrun the durable host-discovery obligation. */
  private async acceptRow(device: string, batch: string, payload: Uint8Array, user: string, chat: string, host: string) {
    const sql = this.ctx.storage.sql;
    const outcome = await this.ctx.storage.transaction(async () => {
      const result = appendRow(sql, device, batch, payload, Date.now());
      if (!result.ok) return result;
      if (!result.dup) {
        this.backupDirty();
        // Rollback gate: a release that reads the stored head must find it
        // current in every room before it runs (see env.ts).
        if (this.env.CHAT_ROOM_WRITE_THROUGH === "1") flushHeadSeq(sql);
        this.flushAt ??= Date.now() + BOOKKEEPING_FLUSH_MS;
      }
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
    const sql = this.ctx.storage.sql;
    // Marked once per backup cycle: the flag only clears together with the
    // deadline, so a room with both is already dirty.
    if (getMeta(sql, "backupDirty") === "1" && getMeta(sql, "backupDue")) return;
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
    const sql = this.ctx.storage.sql;
    const wake = pendingChatWake(sql);
    const backup = getMeta(sql, "backupDirty") === "1"
      ? Number(getMeta(sql, "backupDue") ?? Date.now() + DAY_MS) : Infinity;
    const next = Math.min(wake?.next_at ?? Infinity, backup, this.flushAt ?? Infinity);
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
    const sql = this.ctx.storage.sql;
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

  /** Wake retry, bookkeeping flush and nightly backup share the DO's single
   * durable alarm. */
  async alarm(): Promise<void> {
    try {
      this.flushBookkeeping();
      const wake = pendingChatWake(this.ctx.storage.sql);
      if (wake && wake.next_at <= Date.now()) await this.flushWake();
      const due = Number(getMeta(this.ctx.storage.sql, "backupDue") ?? "0");
      if (due <= Date.now()) {
        try { await this.backup(); }
        catch (error) {
          // Do not exhaust the runtime's finite automatic retries or spin
          // on an expired deadline during an R2 outage.
          setMeta(this.ctx.storage.sql, "backupDue", String(Date.now() + 60_000));
          console.warn("chat2 backup retrying", error);
        }
      }
    } finally {
      await this.ctx.storage.transaction(async () => { await this.scheduleAlarm(); });
    }
  }

  /** Seq-monotonic backup; concurrent new rows retain their dirty flag. */
  private async backup(): Promise<void> {
    const sql = this.ctx.storage.sql;
    if (getMeta(sql, "backupDirty") !== "1") return; // idle: stop the chain
    const head = headSeq(sql);
    if (head > Number(getMeta(sql, "backupSeq") ?? "0")) {
      const rows = [...rowsAfter(sql, 0)].map((row) => ({
        seq: row.seq,
        device: row.device,
        batchId: row.batchId,
        bytes: encodeBase64(row.bytes)
      }));
      // A large checkpoint goes beside the JSON, streamed: inline as
      // base64 it was more than the room could hold in memory.
      const size = this.blobs.size(CHECKPOINT_BLOB) ?? 0;
      const inline = size <= BACKUP_INLINE_CHECKPOINT_BYTES;
      const checkpoint = inline ? this.blobs.get(CHECKPOINT_BLOB) : undefined;
      const prefix = `backup/chat2/${this.ctx.id.toString()}`;
      if (!inline) {
        await this.env.BLOBS.put(
          `${prefix}/checkpoint.bin`,
          this.blobs.stream(CHECKPOINT_BLOB).pipeThrough(new FixedLengthStream(size))
        );
      }
      const frontier = this.blobs.get(FRONTIER_BLOB);
      await this.env.BLOBS.put(
        `${prefix}/latest.json`,
        JSON.stringify({
          at: Date.now(),
          ...logStats(sql),
          checkpoint: checkpoint ? encodeBase64(checkpoint) : null,
          ...(inline ? {} : { checkpointObject: `${prefix}/checkpoint.bin` }),
          frontier: frontier ? encodeBase64(frontier) : null,
          rows
        })
      );
      setMeta(sql, "backupSeq", String(head));
    }
    if (headSeq(sql) === head) {
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
