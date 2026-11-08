/**
 * RegistryRoom — one Durable Object per per-user workspace registry
 * (`reg1/{orgId}/{userId}`), the wedge-proof replacement for the Loro
 * workspace doc (docs/registry-sync.md).
 *
 * The DO is the authority: it stores CURRENT row state in its SQLite (no
 * update log, no replay, no wasm), applies pushed ops with per-field LWW
 * (registry-core.ts), bumps one monotonic `seq` per accepted batch, and
 * broadcasts merged rows to every connected socket. Cold start is a table
 * read — the 2026-07/08 wedge class (CPU-limited history replay) cannot
 * exist here by construction.
 *
 * Persistence model:
 * - `rows` — the state. Written synchronously inside the message event (write
 *   rate is index-scale: renames, status flips — never transcript traffic).
 *   A push writes nothing else: the head is derived from the rows.
 * - `meta` — a lagging seq copy, gcFloor, per-device push attribution, backup
 *   marks. Per-push bookkeeping waits for the alarm's flush (≤ 5 min).
 * - presence — memory-only by construction (15s client beats, rebroadcast).
 *
 * Cloud runners (the Worker stamps RUNNER_DEVICE_HEADER for runner bearers)
 * act as their device, not the user: every row delivery to them is filtered
 * and every op they push must be on a row they host (registry-runner.ts).
 *
 * Hibernation discipline: ZERO wall-clock timers; ping/pong rides the
 * auto-response pair; the one alarm does the bookkeeping flush and, daily,
 * tombstone GC + the R2 backup.
 */
import { applyOp, validateOp, type Op, type Row } from "./registry-core";
import { chatHost, parentOf, runnerCanSee, runnerOpRefusal, type RunnerScope } from "./registry-runner";
import { AUTH_USER_HEADER, RUNNER_ACCOUNT_HEADER, RUNNER_DEVICE_HEADER, apnsConfig, type Env } from "./env";
import { isDeadToken, sendApns, type ApnsEnvironment } from "./apns";
import { WriteMeter } from "./write-meter";
import {
  apnsPayload,
  chatForNotification,
  notificationFor,
  parsePrefs,
  type Category,
  type PushPrefs
} from "./push-notify";

const DAY_MS = 24 * 60 * 60 * 1000;
/** Tombstones older than this are purged; cursors from before the purge
 * horizon get a full resync (`gcFloor`). */
const TOMBSTONE_RETAIN_MS = 30 * DAY_MS;
/** Ops per push batch — a cascade delete of a huge space stays comfortably
 * under this; anything larger is split by the client. */
const MAX_BATCH_OPS = 500;
/** Serialized inbound frame budget. */
const MAX_FRAME_BYTES = 1_000_000;
/** How long per-push bookkeeping (the stored seq copy, per-device push
 * counts) may lag. Writing it on every push was 3 of a
 * one-row push's 5 billed rows; an active room now pays a few rows per
 * flush instead. */
const BOOKKEEPING_FLUSH_MS = 5 * 60_000;

interface SocketState {
  userId: string;
  device: string;
  /** Set once a valid hello established the session. */
  ready?: boolean;
  /** Cloud runner socket: its verified device id. Rows are filtered and
   * pushes ownership-checked for it. */
  runner?: string;
  /** The runner's account (logical Cloud device; JWT `cld`). */
  runnerAccount?: string;
}

/** An op a runner pushed that it does not own — skipped, not fatal. */
interface RejectedOp {
  kind: string;
  id: string;
  error: string;
}

interface WireOp extends Op {}

interface PushOutcome {
  ok: number;
  rejected: number;
  lastOkAt: number;
}

/** A phone that asked for notifications (APNs token + its choices). */
interface PushTarget {
  device: string;
  token: string;
  environment: ApnsEnvironment;
  prefs: PushPrefs;
}

/** One notification decided by a push batch. */
interface Notification {
  chatId: string;
  title: string;
  category: Category;
}

/** Recent deliveries on /stats (the only place to see them). */
interface PushLogEntry {
  at: number;
  chatId: string;
  category: Category;
  device: string;
  result: string;
}

const TOKEN_RE = /^[0-9a-fA-F]{32,512}$/;
const PUSH_LOG_MAX = 30;

export class RegistryRoom implements DurableObject {
  private readonly ctx: DurableObjectState;
  private readonly env: Env;
  /** device → last presence beat (epoch ms). Memory-only. */
  private readonly presence = new Map<string, number>();
  /** Every statement goes through here, so /stats can say what this object
   * has written since it started. */
  private readonly sql: WriteMeter;
  /** device → push counts not yet in `meta.pushOutcomes`; /stats merges
   * them. Eviction can lose the counts since the last flush: diagnostics. */
  private readonly unflushedOutcomes = new Map<string, PushOutcome>();
  /** When the bookkeeping flush is due; undefined = nothing to flush. The
   * alarm it arms is durable, so evicting this field loses no flush. */
  private flushAt?: number;

  constructor(ctx: DurableObjectState, env: Env) {
    this.ctx = ctx;
    this.env = env;
    this.sql = new WriteMeter(ctx.storage.sql);
    this.sql.exec(
      "CREATE TABLE IF NOT EXISTS rows (kind TEXT NOT NULL, id TEXT NOT NULL, seq INTEGER NOT NULL, deleted INTEGER NOT NULL, del_hlc TEXT, fields TEXT NOT NULL, clocks TEXT NOT NULL, PRIMARY KEY (kind, id))"
    );
    this.sql.exec("CREATE INDEX IF NOT EXISTS rows_seq ON rows (seq)");
    this.sql.exec(
      "CREATE TABLE IF NOT EXISTS meta (key TEXT PRIMARY KEY, value TEXT NOT NULL)"
    );
    // Push targets are private to this room: never rows (those broadcast to
    // every socket, sync to every device and back up to R2).
    this.sql.exec(
      "CREATE TABLE IF NOT EXISTS push_targets (device TEXT PRIMARY KEY, token TEXT NOT NULL, environment TEXT NOT NULL, prefs TEXT NOT NULL, updated_at INTEGER NOT NULL)"
    );
    // Same protocol-level keepalive as SessionRoom — and the same caveat: a
    // pong is runtime-answered and proves nothing about this DO's health.
    // Clients judge liveness by probe frames (crates/sync/src/registry.rs).
    ctx.setWebSocketAutoResponse(new WebSocketRequestResponsePair("ping", "pong"));
  }

  // ── meta helpers ──────────────────────────────────────────────────────────

  private getMeta(key: string): string | undefined {
    const rows = [...this.sql.exec("SELECT value FROM meta WHERE key = ?", key)];
    return rows[0]?.value as string | undefined;
  }

  private setMeta(key: string, value: string): void {
    this.sql.exec(
      "INSERT INTO meta (key, value) VALUES (?, ?) ON CONFLICT(key) DO UPDATE SET value = excluded.value",
      key,
      value
    );
  }

  /** The room's head comes from what the table holds, so it can never
   * disagree with the rows: the highest row's seq, or the GC floor when
   * tombstone GC removed the top rows (the floor is their max seq). Neither
   * can regress. `meta.seq` is a lagging copy for older releases (see
   * `flushBookkeeping`); `/reset` clears both, so the head drops to 0 there
   * as before. */
  private seq(): number {
    const top = [...this.sql.exec("SELECT MAX(seq) AS top FROM rows")][0]?.top as number | null;
    return Math.max(top ?? 0, this.gcFloor(), Number(this.getMeta("seq") ?? "0"));
  }

  private gcFloor(): number {
    return Number(this.getMeta("gcFloor") ?? "0");
  }

  // ── row storage ───────────────────────────────────────────────────────────

  private loadRow(kind: string, id: string): Row | undefined {
    const rows = [
      ...this.sql.exec(
        "SELECT seq, deleted, del_hlc, fields, clocks FROM rows WHERE kind = ? AND id = ?",
        kind,
        id
      )
    ];
    const raw = rows[0];
    if (!raw) return undefined;
    return {
      kind,
      id,
      seq: raw.seq as number,
      deleted: (raw.deleted as number) === 1,
      ...(raw.del_hlc ? { delHlc: raw.del_hlc as string } : {}),
      fields: JSON.parse(raw.fields as string) as Row["fields"],
      clocks: JSON.parse(raw.clocks as string) as Row["clocks"]
    };
  }

  private saveRow(row: Row): void {
    this.sql.exec(
      "INSERT INTO rows (kind, id, seq, deleted, del_hlc, fields, clocks) VALUES (?, ?, ?, ?, ?, ?, ?) ON CONFLICT(kind, id) DO UPDATE SET seq = excluded.seq, deleted = excluded.deleted, del_hlc = excluded.del_hlc, fields = excluded.fields, clocks = excluded.clocks",
      row.kind,
      row.id,
      row.seq,
      row.deleted ? 1 : 0,
      row.delHlc ?? null,
      JSON.stringify(row.fields),
      JSON.stringify(row.clocks)
    );
  }

  /** What one reader may receive: everything for user callers, owned rows
   * (plus all devices, and the projects of the chats it hosts) for a runner. */
  private visible(rows: Row[], runner: string | undefined, _account: string | undefined): Row[] {
    if (runner === undefined) return rows;
    const scope = this.runnerScope(runner);
    return rows.filter((row) => runnerCanSee(runner, row, scope));
  }

  /** What `device` reaches beyond its own rows: the projects of the live
   * chats it hosts, and the live chats whose parent it hosts (delegates it
   * started on other machines). */
  private runnerScope(device: string): RunnerScope {
    const spaces = new Set<string>();
    const hosted = new Set<string>();
    const parents: [string, string][] = [];
    for (const raw of this.sql.exec<{ id: string; fields: string }>(
      "SELECT id, fields FROM rows WHERE kind = 'chats' AND deleted = 0"
    )) {
      try {
        const fields = JSON.parse(raw.fields) as Record<string, unknown>;
        if (fields.deviceId === device) {
          hosted.add(raw.id);
          if (typeof fields.spaceId === "string") spaces.add(fields.spaceId);
        }
        if (typeof fields.parentChatId === "string") parents.push([raw.id, fields.parentChatId]);
      } catch {
        /* a malformed row names no project and no parent */
      }
    }
    const children = new Set(parents.filter(([, parent]) => hosted.has(parent)).map(([id]) => id));
    return { spaces, children };
  }

  /** The hosts of the parents of the live chats `device` hosts: the runners
   * that delegated work to it (the Worker's device-room gate). */
  private parentHosts(device: string): string[] {
    const hosts = new Set<string>();
    for (const raw of this.sql.exec<{ fields: string }>(
      "SELECT fields FROM rows WHERE kind = 'chats' AND deleted = 0"
    )) {
      try {
        const fields = JSON.parse(raw.fields) as Record<string, unknown>;
        if (fields.deviceId !== device || typeof fields.parentChatId !== "string") continue;
        const host = chatHost(this.loadRow("chats", fields.parentChatId));
        if (host) hosts.add(host);
      } catch {
        /* a malformed row delegates nothing */
      }
    }
    return [...hosts];
  }

  private rowsSince(cursor: number): Row[] {
    const out: Row[] = [];
    for (const raw of this.sql.exec(
      "SELECT kind, id, seq, deleted, del_hlc, fields, clocks FROM rows WHERE seq > ? ORDER BY seq",
      cursor
    )) {
      out.push({
        kind: raw.kind as string,
        id: raw.id as string,
        seq: raw.seq as number,
        deleted: (raw.deleted as number) === 1,
        ...(raw.del_hlc ? { delHlc: raw.del_hlc as string } : {}),
        fields: JSON.parse(raw.fields as string) as Row["fields"],
        clocks: JSON.parse(raw.clocks as string) as Row["clocks"]
      });
    }
    return out;
  }

  // ── HTTP surface (only reachable through the authed Worker; org membership
  //    and the per-user room name were enforced there) ──────────────────────

  async fetch(request: Request): Promise<Response> {
    const url = new URL(request.url);
    const userId = request.headers.get(AUTH_USER_HEADER);
    if (!userId) return json({ error: "unauthenticated" }, 401);
    const runner = request.headers.get(RUNNER_DEVICE_HEADER) || undefined;
    const runnerAccount = runner === undefined ? undefined : request.headers.get(RUNNER_ACCOUNT_HEADER) || undefined;
    // Defense in depth (the Worker refuses these too): a runner syncs rows
    // and nothing else — no stats (push targets, push log), no APNs target
    // registration (notification titles of every chat), no operator wipe.
    if (runner !== undefined && !["/ws", "/rows", "/push"].includes(url.pathname)) {
      return json({ error: "forbidden" }, 403);
    }

    if (url.pathname === "/ws") {
      // A runner's device identity is the verified one, never self-declared.
      const device = runner ?? url.searchParams.get("device") ?? "";
      const pair = new WebSocketPair();
      this.ctx.acceptWebSocket(pair[1]);
      const state: SocketState = {
        userId,
        device,
        ...(runner ? { runner } : {}),
        ...(runnerAccount ? { runnerAccount } : {})
      };
      pair[1].serializeAttachment(state);
      return new Response(null, { status: 101, webSocket: pair[0] });
    }

    if (url.pathname === "/stats" && request.method === "GET") {
      const rowCount = [...this.sql.exec("SELECT COUNT(*) AS n FROM rows")][0]
        ?.n as number;
      const tombstones = [
        ...this.sql.exec("SELECT COUNT(*) AS n FROM rows WHERE deleted = 1")
      ][0]?.n as number;
      return json({
        seq: this.seq(),
        gcFloor: this.gcFloor(),
        rowCount,
        tombstones,
        connectedSockets: this.ctx.getWebSockets().length,
        // The ONLY per-device attribution surface — kept from the 2026-08-05
        // incident tooling (SessionRoom's /stats pushOutcomes).
        pushOutcomes: this.mergedOutcomes(),
        lastBackupSeq: Number(this.getMeta("backupSeq") ?? "0"),
        lastGcAt: Number(this.getMeta("lastGcAt") ?? "0"),
        pushTargets: this.pushTargets().map((t) => ({ device: t.device, environment: t.environment, prefs: t.prefs })),
        pushConfigured: apnsConfig(this.env) !== undefined,
        pushLog: JSON.parse(this.getMeta("pushLog") ?? "[]") as PushLogEntry[],
        // Billed rows (index entries and alarms included) since this object
        // last started.
        writesSinceStart: this.sql.snapshot()
      });
    }

    // A phone registering (or updating) its APNs token and choices; DELETE on
    // sign-out or when the user turns notifications off.
    if (url.pathname === "/push-target") {
      const device = url.searchParams.get("device") ?? "";
      if (device === "") return json({ error: "bad_request", message: "device required" }, 400);
      if (request.method === "DELETE") {
        this.sql.exec("DELETE FROM push_targets WHERE device = ?", device);
        return json({ ok: true });
      }
      if (request.method === "POST") {
        let body: Record<string, unknown>;
        try {
          body = (await request.json()) as Record<string, unknown>;
        } catch {
          return json({ error: "bad_request", message: "malformed body" }, 400);
        }
        const token = typeof body.token === "string" ? body.token : "";
        const environment = body.environment === "sandbox" ? "sandbox" : body.environment === "production" ? "production" : undefined;
        if (!TOKEN_RE.test(token) || environment === undefined) {
          return json({ error: "bad_request", message: "token and environment required" }, 400);
        }
        this.sql.exec(
          "INSERT INTO push_targets (device, token, environment, prefs, updated_at) VALUES (?, ?, ?, ?, ?) ON CONFLICT(device) DO UPDATE SET token = excluded.token, environment = excluded.environment, prefs = excluded.prefs, updated_at = excluded.updated_at",
          device,
          token,
          environment,
          JSON.stringify(parsePrefs(body.prefs)),
          Date.now()
        );
        return json({ ok: true });
      }
    }

    if (url.pathname === "/rows" && request.method === "GET") {
      // Pull over plain HTTPS: with `?since=` this is the WS hello's exact
      // delta answer (same full/gcFloor rules); without it, the original
      // full-table repair read. `?device=&beat=1` doubles as a presence
      // beat so an HTTP-only client stays visible to socket peers.
      const sinceRaw = url.searchParams.get("since");
      const sinceNum = sinceRaw === null ? NaN : Number(sinceRaw);
      const cursor = Number.isInteger(sinceNum) && sinceNum >= 0 ? sinceNum : null;
      const device = runner ?? url.searchParams.get("device") ?? "";
      if (device !== "" && url.searchParams.get("beat") === "1") {
        const at = Date.now();
        this.presence.set(device, at);
        for (const socket of this.ctx.getWebSockets()) {
          const socketState = socket.deserializeAttachment() as SocketState | null;
          if (!socketState?.ready) continue;
          send(socket, { t: "presence", device, at });
        }
      }
      const seq = this.seq();
      const gcFloor = this.gcFloor();
      const full = cursor === null || cursor < gcFloor || cursor > seq;
      return json({
        seq,
        full,
        gcFloor,
        rows: this.visible(full ? this.rowsSince(0) : this.rowsSince(cursor), runner, runnerAccount),
        presence: Object.fromEntries(this.presence)
      });
    }

    // The Worker's chat2/blob gate for runners: who hosts this chat? Only
    // reachable through the Worker, which asks the caller's own room.
    if (url.pathname === "/chat-host" && request.method === "GET") {
      const chat = url.searchParams.get("chat") ?? "";
      const row = this.loadRow("chats", chat);
      const parent = parentOf(row);
      return json({
        deviceId: chatHost(row) ?? null,
        // A delegate's parent host may reach it too (its runner started it).
        parentHost: (parent && chatHost(this.loadRow("chats", parent))) ?? null,
        // Whether the id was ever used, tombstones included: a new delegate
        // must take a fresh id, never an existing (or deleted) chat's.
        known: row !== undefined
      });
    }

    // The Worker's device-room gate for runners: which runners delegated
    // work to this device (host the parents of the chats it hosts)?
    if (url.pathname === "/device-parents" && request.method === "GET") {
      const device = url.searchParams.get("device") ?? "";
      return json({ hosts: this.parentHosts(device) });
    }

    if (url.pathname === "/push" && request.method === "POST") {
      // Push over plain HTTPS — the WS push's fallback twin for networks
      // where the upgrade never completes. Same validation, same atomic
      // apply, same rows broadcast to live sockets; the ack is the response
      // body. LWW clocks make replayed batches apply zero ops, so
      // at-least-once delivery (including 0-RTT/early-data replays) is safe.
      const device = runner ?? url.searchParams.get("device") ?? "";
      // Pre-read cap (the WS path gets this for free from MAX_FRAME_BYTES):
      // a legitimate batch is bounded well under this by MAX_BATCH_OPS ×
      // MAX_OP_BYTES; anything larger only burns this room's own CPU.
      const declared = Number(request.headers.get("content-length") ?? "0");
      if (declared > 2 * 1024 * 1024) return json({ error: "too_large" }, 413);
      let frame: Record<string, unknown>;
      try {
        frame = (await request.json()) as Record<string, unknown>;
      } catch {
        return json({ error: "bad_push", message: "malformed body" }, 400);
      }
      const outcome = this.applyPushBatch(device, frame, runner);
      if (!outcome.ok) return json({ error: outcome.code, message: outcome.message }, 400);
      return json({
        batch: outcome.batch,
        seq: outcome.seq,
        applied: outcome.applied,
        ...(outcome.rejected.length > 0 ? { rejected: outcome.rejected } : {})
      });
    }

    if (url.pathname === "/reset" && request.method === "POST") {
      // Operator wipe. Recovery is automatic and lossless: every client
      // detects `state.seq < cursor` on its next hello and re-seeds the table
      // from its local rows with their ORIGINAL clocks (registry-core
      // rowToSeedOp) — the ws4 repair recipe, built in.
      this.sql.exec("DELETE FROM rows");
      this.sql.exec("DELETE FROM meta");
      this.unflushedOutcomes.clear();
      this.flushAt = undefined;
      for (const ws of this.ctx.getWebSockets()) {
        try {
          ws.close(4410, "registry reset");
        } catch {
          /* already gone */
        }
      }
      return json({ ok: true });
    }

    return json({ error: "not found" }, 404);
  }

  // ── WebSocket protocol ────────────────────────────────────────────────────

  async webSocketMessage(ws: WebSocket, message: ArrayBuffer | string): Promise<void> {
    if (typeof message !== "string") {
      ws.close(1003, "text frames only");
      return;
    }
    if (message.length > MAX_FRAME_BYTES) {
      ws.close(1009, "frame too large");
      return;
    }
    let frame: Record<string, unknown>;
    try {
      frame = JSON.parse(message) as Record<string, unknown>;
    } catch {
      ws.close(1002, "bad json");
      return;
    }
    const state = ws.deserializeAttachment() as SocketState;
    switch (frame.t) {
      case "hello":
        this.handleHello(ws, state, frame);
        return;
      case "push":
        this.handlePush(ws, state, frame);
        return;
      case "presence":
        this.handlePresence(ws, state, frame);
        return;
      case "probe":
        send(ws, { t: "probe-ok", seq: this.seq() });
        return;
      default:
        send(ws, { t: "error", code: "bad_frame", message: `unknown frame: ${String(frame.t)}` });
    }
  }

  async webSocketClose(): Promise<void> {
    /* nothing buffered; rows are written synchronously on push */
  }

  async webSocketError(): Promise<void> {
    /* ditto */
  }

  private handleHello(ws: WebSocket, state: SocketState, frame: Record<string, unknown>): void {
    const cursor = typeof frame.cursor === "number" && frame.cursor >= 0 ? frame.cursor : null;
    if (state.runner === undefined && typeof frame.device === "string" && frame.device.length > 0) {
      state.device = frame.device;
    }
    state.ready = true;
    ws.serializeAttachment(state);
    const seq = this.seq();
    const gcFloor = this.gcFloor();
    // A cursor from before the tombstone-GC horizon may have missed purged
    // deletes; a cursor AHEAD of the server means the server lost state (a
    // reset/wipe) — both force `full`, and the client reacts by replacing
    // (former) or re-seeding the server (latter; see registry.rs).
    const full = cursor === null || cursor < gcFloor || cursor > seq;
    const rows = this.visible(full ? this.rowsSince(0) : this.rowsSince(cursor), state.runner, state.runnerAccount);
    send(ws, {
      t: "state",
      seq,
      full,
      gcFloor,
      rows,
      presence: Object.fromEntries(this.presence)
    });
  }

  private handlePush(ws: WebSocket, state: SocketState, frame: Record<string, unknown>): void {
    if (!state.ready) {
      this.recordPush(state.device, false);
      send(ws, { t: "error", code: "bad_push", message: "hello first / malformed push" });
      return;
    }
    const outcome = this.applyPushBatch(state.device, frame, state.runner);
    if (!outcome.ok) {
      send(ws, { t: "error", code: outcome.code, message: outcome.message });
      return;
    }
    // Skipped runner ops are reported like invalid ones (error frames the
    // client logs and counts); the ack still retires the batch.
    for (const rejected of outcome.rejected.slice(0, 20)) {
      send(ws, { t: "error", code: "not_owner", message: `${rejected.kind}/${rejected.id}: ${rejected.error}` });
    }
    send(ws, {
      t: "ack",
      batch: outcome.batch,
      seq: outcome.seq,
      applied: outcome.applied,
      ...(outcome.rejected.length > 0 ? { rejected: outcome.rejected } : {})
    });
  }

  /** Validate + atomically apply one op batch and broadcast merged rows to
   * every ready socket -- shared by the WS push and the HTTP `POST /push`
   * fallback. The caller delivers the ack/error on its own transport.
   * `runner` (a Cloud device) may only touch rows it hosts: its other ops
   * are skipped and reported in `rejected`, the rest of the batch applies. */
  private applyPushBatch(
    device: string,
    frame: Record<string, unknown>,
    runner?: string
  ):
    | { ok: false; code: string; message: string }
    | { ok: true; batch: string; seq: number; applied: number; rejected: RejectedOp[] } {
    const batch = typeof frame.batch === "string" ? frame.batch : "";
    if (batch === "" || !Array.isArray(frame.ops)) {
      this.recordPush(device, false);
      return { ok: false, code: "bad_push", message: "malformed push" };
    }
    const ops = frame.ops as WireOp[];
    if (ops.length === 0 || ops.length > MAX_BATCH_OPS) {
      this.recordPush(device, false);
      return { ok: false, code: "bad_push", message: `batch of ${ops.length} ops` };
    }
    for (const op of ops) {
      const invalid = validateOp(op);
      if (invalid) {
        // Reject the WHOLE batch: batches are transactional (cascade deletes)
        // and a client that builds one bad op is a client bug to surface, not
        // to partially apply. Rejections are attributed per device on /stats.
        this.recordPush(device, false);
        return { ok: false, code: "invalid_op", message: `${op.kind}/${op.id}: ${invalid}` };
      }
    }

    // Apply atomically: DO events are single-threaded and SQLite writes in an
    // event commit together, so a mid-batch crash never persists half a batch.
    const nextSeq = this.seq() + 1;
    const touched = new Map<string, Row>();
    /** Each touched session row as it was before this batch. */
    const sessionsBefore = new Map<string, Row | undefined>();
    let applied = 0;
    const rejected: RejectedOp[] = [];
    // Children as the batch starts: a delegate the runner may manage.
    const children = runner === undefined ? undefined : this.runnerScope(runner).children;
    for (const op of ops) {
      const key = `${op.kind} ${op.id}`;
      const before = touched.get(key) ?? this.loadRow(op.kind, op.id);
      if (runner !== undefined) {
        const refusal = runnerOpRefusal(runner, before, op, children);
        if (refusal !== null) {
          rejected.push({ kind: op.kind, id: op.id, error: refusal });
          continue;
        }
      }
      if (op.kind === "sessions" && !sessionsBefore.has(op.id)) sessionsBefore.set(op.id, before);
      const { row, changed } = applyOp(before, op);
      if (!changed || row === undefined) continue;
      applied += 1;
      row.seq = nextSeq;
      touched.set(key, row);
    }
    if (applied > 0) {
      // The rows are the only thing a push must store: their seq IS the new
      // head (see `seq`). One event's writes commit together.
      for (const row of touched.values()) this.saveRow(row);
      // Rollback gate: a release that reads the stored seq must find it
      // current in every room before it runs (see env.ts).
      if (this.env.REGISTRY_ROOM_WRITE_THROUGH === "1") this.setMeta("seq", String(nextSeq));
      // The backup deadline is a day out and the flush recordPush arms is
      // sooner; the flush re-arms the alarm for the backup.
      this.markBackupDirty();
    }
    this.recordPush(device, true);
    if (rejected.length > 0) this.recordPush(device, false);
    const seq = applied > 0 ? nextSeq : this.seq();
    if (applied > 0) {
      // Merged full rows to EVERY ready socket (sender included -- its op may
      // have lost LWW, and the merged row is the truth it must display).
      // Rows go out BEFORE the ack so the sender's authoritative state is
      // current by the time the ack retires its optimistic pending batch --
      // no between-frames flicker window.
      const rows = [...touched.values()];
      for (const socket of this.ctx.getWebSockets()) {
        const socketState = socket.deserializeAttachment() as SocketState | null;
        if (!socketState?.ready) continue;
        // A runner still gets the frame (possibly empty) so its cursor
        // advances with the room's seq.
        send(socket, { t: "rows", seq, rows: this.visible(rows, socketState.runner, socketState.runnerAccount) });
      }
      this.notifySessions(sessionsBefore, touched);
    }
    return { ok: true, batch, seq, applied, rejected };
  }

  // ── push notifications ───────────────────────────────────────────────────

  private pushTargets(): PushTarget[] {
    return [...this.sql.exec("SELECT device, token, environment, prefs FROM push_targets")].map((raw) => ({
      device: raw.device as string,
      token: raw.token as string,
      environment: (raw.environment as string) === "sandbox" ? "sandbox" : "production",
      prefs: parsePrefs(JSON.parse(raw.prefs as string))
    }));
  }

  /** Session rows that changed in a batch → the desktop's notifications
   * (done / needs input / failed), sent to every registered phone after the
   * batch commits. Best effort: a failed delivery is logged, never retried. */
  private notifySessions(before: Map<string, Row | undefined>, touched: Map<string, Row>): void {
    if (before.size === 0) return;
    const now = Date.now();
    const notes: Notification[] = [];
    for (const [chatId, prev] of before) {
      const next = touched.get(`sessions ${chatId}`);
      if (!next) continue;
      const category = notificationFor(prev, next, now);
      if (category === null) continue;
      const chat = chatForNotification(touched.get(`chats ${chatId}`) ?? this.loadRow("chats", chatId));
      if (chat === null) continue;
      notes.push({ chatId, title: chat.title, category });
    }
    if (notes.length === 0) return;
    const targets = this.pushTargets();
    const config = apnsConfig(this.env);
    const log: PushLogEntry[] = [];
    const sends: Array<() => Promise<void>> = [];
    for (const note of notes) {
      for (const target of targets) {
        if (!target.prefs[note.category]) continue;
        const entry: PushLogEntry = { at: now, chatId: note.chatId, category: note.category, device: target.device, result: "pending" };
        log.push(entry);
        if (config === undefined) {
          entry.result = "not configured";
          continue;
        }
        sends.push(async () => {
          try {
            const r = await sendApns(config, target.environment, target.token, apnsPayload(note.chatId, note.title, note.category), note.chatId);
            entry.result = r.reason ? `${r.status} ${r.reason}` : String(r.status);
            if (isDeadToken(r)) {
              this.sql.exec("DELETE FROM push_targets WHERE device = ? AND token = ?", target.device, target.token);
            }
          } catch (err) {
            entry.result = `error ${(err as Error).message}`;
          }
        });
      }
    }
    if (targets.length === 0) {
      for (const note of notes) log.push({ at: now, chatId: note.chatId, category: note.category, device: "", result: "no targets" });
    }
    // One stored write per batch that notifies: with deliveries to make, the
    // entries go in once they carry their results.
    if (sends.length > 0) {
      this.ctx.waitUntil(Promise.all(sends.map((send) => send())).then(() => this.appendPushLog(log)));
    } else {
      this.appendPushLog(log);
    }
  }

  /** Keep the last few deliveries; `updated` entries (same objects, results
   * filled in after sending) replace their pending copies. */
  private appendPushLog(entries: PushLogEntry[]): void {
    const log = JSON.parse(this.getMeta("pushLog") ?? "[]") as PushLogEntry[];
    this.setMeta("pushLog", JSON.stringify([...log, ...entries].slice(-PUSH_LOG_MAX)));
  }

  private handlePresence(ws: WebSocket, state: SocketState, frame: Record<string, unknown>): void {
    if (!state.ready || state.device === "") return;
    const at = typeof frame.at === "number" ? frame.at : Date.now();
    this.presence.set(state.device, at);
    for (const socket of this.ctx.getWebSockets()) {
      if (socket === ws) continue;
      const socketState = socket.deserializeAttachment() as SocketState | null;
      if (!socketState?.ready) continue;
      send(socket, { t: "presence", device: state.device, at });
    }
  }

  /** Per-device attribution on /stats. An accepted push is counted in
   * memory and stored by the flush, so its count and `lastOkAt` are best
   * effort: an object evicted before its flush (an idle room hibernates
   * within seconds) loses them. A rejection, the signal the 2026-08-05
   * incident was debugged by, is rare and stored at once with the rest. */
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
    if (ok) {
      this.armFlush();
    } else {
      this.setMeta("pushOutcomes", JSON.stringify(this.mergedOutcomes()));
      this.unflushedOutcomes.clear();
    }
  }

  /** Stored push counts plus the ones still in memory. */
  private mergedOutcomes(): Record<string, PushOutcome> {
    const outcomes = JSON.parse(this.getMeta("pushOutcomes") ?? "{}") as Record<string, PushOutcome>;
    for (const [key, delta] of this.unflushedOutcomes) {
      const entry = outcomes[key] ?? { ok: 0, rejected: 0, lastOkAt: 0 };
      entry.ok += delta.ok;
      entry.rejected += delta.rejected;
      entry.lastOkAt = Math.max(entry.lastOkAt, delta.lastOkAt);
      outcomes[key] = entry;
    }
    return outcomes;
  }

  /** Arm the bookkeeping flush unless it already is. */
  private armFlush(): void {
    if (this.flushAt !== undefined) return;
    this.flushAt = Date.now() + BOOKKEEPING_FLUSH_MS;
    void this.scheduleAlarm();
  }

  /** Store what pushes only kept in memory: the seq copy older releases
   * read, and the accepted-push counts. Each only when it changed. */
  private flushBookkeeping(): void {
    const head = this.seq();
    if (head !== Number(this.getMeta("seq") ?? "0")) this.setMeta("seq", String(head));
    if (this.unflushedOutcomes.size > 0) {
      this.setMeta("pushOutcomes", JSON.stringify(this.mergedOutcomes()));
      this.unflushedOutcomes.clear();
    }
    this.flushAt = undefined;
  }

  /** Mark the table for the nightly backup, once per backup cycle: the flag
   * and its deadline only clear together, so a room with both is already
   * marked. */
  private markBackupDirty(): void {
    if (this.getMeta("backupDirty") !== "1") this.setMeta("backupDirty", "1");
    if (this.getMeta("backupDue") === undefined) this.setMeta("backupDue", String(Date.now() + DAY_MS));
  }

  /** The one durable alarm serves the bookkeeping flush and the nightly
   * backup: arm it for whichever is due first, unless it already is. */
  private async scheduleAlarm(): Promise<void> {
    const backup =
      this.getMeta("backupDirty") === "1" ? Number(this.getMeta("backupDue") ?? Date.now() + DAY_MS) : Infinity;
    const next = Math.min(backup, this.flushAt ?? Infinity);
    if (!Number.isFinite(next)) return;
    const existing = await this.ctx.storage.getAlarm();
    if (existing !== null && existing <= next) return;
    this.sql.alarm();
    await this.ctx.storage.setAlarm(next);
  }

  /** The flush, and once a day the tombstone GC + R2 backup of the table. */
  async alarm(): Promise<void> {
    this.flushBookkeeping();
    const due = Number(this.getMeta("backupDue") ?? "0");
    // A room marked by a release before the deadline existed has no
    // deadline: its alarm was armed for the backup, so back up now.
    if (this.getMeta("backupDirty") === "1" && due <= Date.now()) await this.backup();
    // Idle: nothing re-arms, and the chain stops.
    await this.scheduleAlarm();
  }

  private async backup(): Promise<void> {
    // 1. Tombstone GC. Raising gcFloor to the purged rows' max seq forces a
    //    full resync for any cursor that might have missed a purged delete.
    const horizon = Date.now() - TOMBSTONE_RETAIN_MS;
    const horizonHlc = `${String(horizon).padStart(13, "0")}-`;
    let purgedMaxSeq = 0;
    for (const raw of this.sql.exec(
      "SELECT seq FROM rows WHERE deleted = 1 AND del_hlc < ?",
      horizonHlc
    )) {
      purgedMaxSeq = Math.max(purgedMaxSeq, raw.seq as number);
    }
    if (purgedMaxSeq > 0) {
      this.sql.exec("DELETE FROM rows WHERE deleted = 1 AND del_hlc < ?", horizonHlc);
      this.setMeta("gcFloor", String(Math.max(this.gcFloor(), purgedMaxSeq)));
      this.setMeta("lastGcAt", String(Date.now()));
    }

    // 2. Nightly R2 backup — monotonic by seq, so a wiped-and-reseeding room
    //    can never replace the last good copy with a hollow one.
    const seq = this.seq();
    if (seq > Number(this.getMeta("backupSeq") ?? "0")) {
      const rows = this.rowsSince(0);
      await this.env.BLOBS.put(
        `backup/registry/${this.ctx.id.toString()}/latest.json`,
        JSON.stringify({ seq, at: Date.now(), rows })
      );
      this.setMeta("backupSeq", String(seq));
    }
    this.setMeta("backupDirty", "0");
    this.sql.exec("DELETE FROM meta WHERE key = 'backupDue'");
  }
}

const send = (ws: WebSocket, frame: Record<string, unknown>): void => {
  try {
    ws.send(JSON.stringify(frame));
  } catch {
    /* socket already gone; hibernation API cleans it up */
  }
};

const json = (value: unknown, status = 200): Response =>
  new Response(JSON.stringify(value), {
    status,
    headers: { "content-type": "application/json" }
  });
