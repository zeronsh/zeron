import type { ConnectivityState, ContextUsage, SessionMessageEntry, TranscriptFrame, TranscriptUpdate } from "@zeron/proto";
import type { EngineClient, WatchHandle } from "@zeron/engine-client";
import { methods, RpcError } from "@zeron/engine-client";
import { mintId } from "../lib/id";
import {
  PendingQueuedTurns,
  SavedViewportCache,
  applyTranscriptFrame,
  TranscriptDesync,
} from "../lib/transcript";

/**
 * One chat's live transcript — the store behind the transcript view. Subscribes
 * `WatchDocMessages {chatId}` (which also serves subagent docs: the doc id is
 * the parameter, so the same store backs the subagent dialog), applies
 * reset/delta frames through the ported `applyTranscriptFrame`, and resubscribes
 * for a fresh reset when the desync tripwire fires.
 *
 * Same React-binding contract as the watch cache: `getSnapshot()` is
 * identity-stable until an actual change, `subscribe` fires once per change.
 */

/**
 * `TranscriptReplayState` (transcript.rs:2426): a saved viewport is restored
 * only after a populated replay — an `empty` replay is authoritative (the
 * chat really has no messages) and `pending` is neither.
 */
export type TranscriptReplayState = "pending" | "empty" | "populated";

/**
 * One accepted replay baseline (ticket 69): the durable, store-owned identity
 * of a reset boundary. Published ONLY when a reset has actually been accepted
 * and applied — the offline cache seed and each authoritative live reset
 * frame (same- or new-generation alike). Stale generations, malformed frames,
 * deltas, and bare resubscribe windows never publish one. The surface
 * consumes each epoch exactly once, so baseline recognition no longer depends
 * on React ever rendering an intermediate `pending` snapshot (a generation
 * swap commits pending and populated in one task).
 */
export interface TranscriptBaseline {
  /** Store-local monotonic identity — 1, 2, 3… in acceptance order. */
  readonly epoch: number;
  /** `seed`: the offline cache's last-seen entries; `reset`: an authoritative live reset frame. */
  readonly provenance: "seed" | "reset";
  /** The entries exactly as accepted — the surface re-derives the baseline rows from these. */
  readonly entries: readonly SessionMessageEntry[];
}

export interface TranscriptSnapshot {
  /** The transcript entries in document order (immutable, identity-preserving). */
  readonly entries: readonly SessionMessageEntry[];
  /** The host-owned context snapshot riding the stream (null on older engines). */
  readonly contextUsage: ContextUsage | null;
  /** A first frame has arrived on the current stream. */
  readonly loaded: boolean;
  /** The last entry is streaming (drives the live-end anchor). */
  readonly streaming: boolean;
  /** Terminal stream error on the current generation (Retry re-subscribes). */
  readonly error: string | null;
  /** The connection generation these rows belong to. */
  readonly generation: number;
  /** Where the replay stands (`TranscriptReplayState`). */
  readonly replay: TranscriptReplayState;
  /** The latest accepted replay baseline (`TranscriptBaseline`), if any. */
  readonly baseline: TranscriptBaseline | null;
}

/** Seeds hydrate offscreen; an accepted live reset or offline error permits presentation. */
export function transcriptSnapshotIsLive(snapshot: TranscriptSnapshot): boolean {
  return snapshot.baseline?.provenance === "reset" || snapshot.error !== null;
}

const EMPTY_ENTRIES: readonly SessionMessageEntry[] = [];

// ---------------------------------------------------------------------------
// Optimistic echo / pending sends
// ---------------------------------------------------------------------------

/**
 * A send the user has made that the real transcript has not confirmed yet —
 * the desktop's `PendingSend` (`crates/ui/src/state.rs:907-909`), rendered as
 * an echo bubble (`push_echo`/`remove_echo`, `:1111-1128`) so the message is on
 * screen the instant it is sent rather than whenever the engine gets round to
 * writing it back.
 */
export interface PendingSend {
  readonly messageId: string;
  readonly chatId: string;
  readonly startedAtMs: number;
  readonly text: string;
  readonly attachmentPaths: readonly string[];
}

/** `state.rs`'s `UNDELIVERED_GRACE_MS` — 120s before a send is called failed. */
export const UNDELIVERED_GRACE_MS = 120_000;

export type PendingSendStatus = "pending" | "undelivered";

/**
 * `chat_delivery_degraded`'s web arm (state.rs:877-902), deliberately
 * minimal: the routed engine's `WatchConnectivity` posture decides —
 * Offline/Reconnecting degrade delivery, Connected and Disabled do not
 * (the desktop's `Disabled => false` early return), and an unobserved
 * slot (null) does not either. The desktop's per-chat room map and
 * device-presence arms stay unported: one engine's own stream is the only
 * delivery path the web client holds, and the chat page threads this
 * value from the session's watch cache (ticket 30's per-engine slot;
 * ticket 31's routing picks the chat's engine).
 */
export function chatDeliveryDegraded(state: ConnectivityState | null | undefined): boolean {
  return state === "offline" || state === "reconnecting";
}

/**
 * `send_pending` / `send_undelivered` (`state.rs:1136-1157,1227-1239`): inside
 * the grace window a send is merely pending — quiet, not alarming; past it,
 * with nothing confirming it, it is explicitly undelivered and offers a retry.
 *
 * `degraded` is the AND-in point for `chat_delivery_degraded` (above):
 * degraded delivery keeps the send pending however long it has waited —
 * the honest state is "Queued", never a false "Not delivered" during an
 * outage the engine itself has already reported.
 */
export function pendingSendStatus(
  send: PendingSend,
  nowMs: number,
  degraded = false,
): PendingSendStatus {
  if (degraded) {
    return "pending";
  }
  return nowMs - send.startedAtMs <= UNDELIVERED_GRACE_MS ? "pending" : "undelivered";
}

const NO_SENDS: readonly PendingSend[] = [];

/**
 * The app's echo overlay: chat id → the sends still awaiting confirmation.
 *
 * It is module-scoped rather than a `TranscriptStore` field on purpose. A
 * `TranscriptStore` is created per open chat and disposed on every chat
 * switch, so pending sends living inside one would vanish the moment the user
 * looked at another chat — the desktop keeps them on `AppState`, which
 * outlives any one transcript.
 *
 * EVERYTHING here is keyed by `messageId`, never by `chatId` alone: two sends
 * can be in flight in the same chat, and one failing must not clear the other
 * (`send_failure_cleanup_only_ends_its_own_overlay`).
 */
export class EchoStore {
  #byChat = new Map<string, readonly PendingSend[]>();
  readonly #listeners = new Set<() => void>();

  /** The sends awaiting confirmation in one chat, oldest first. */
  forChat(chatId: string): readonly PendingSend[] {
    return this.#byChat.get(chatId) ?? NO_SENDS;
  }

  get(messageId: string): PendingSend | null {
    for (const sends of this.#byChat.values()) {
      const hit = sends.find((send) => send.messageId === messageId);
      if (hit !== undefined) {
        return hit;
      }
    }
    return null;
  }

  subscribe(listener: () => void): () => void {
    this.#listeners.add(listener);
    return () => {
      this.#listeners.delete(listener);
    };
  }

  /** Show an echo for a just-sent message (`push_echo` + `begin_pending_send`). */
  pushEcho(send: PendingSend): void {
    const sends = this.forChat(send.chatId);
    if (sends.some((existing) => existing.messageId === send.messageId)) {
      return;
    }
    this.#byChat.set(send.chatId, [...sends, send]);
    this.#emit();
  }

  /**
   * Drop ONE send's echo — the ack, and the failure cleanup. Silent when the
   * id is unknown (a doubled ack, or a send that already landed).
   */
  removeEcho(messageId: string): void {
    for (const [chatId, sends] of this.#byChat) {
      const next = sends.filter((send) => send.messageId !== messageId);
      if (next.length === sends.length) {
        continue;
      }
      if (next.length === 0) {
        this.#byChat.delete(chatId);
      } else {
        this.#byChat.set(chatId, next);
      }
      this.#emit();
      return;
    }
  }

  /**
   * `ack_pending_send_from_transcript`: the instant a message id shows up in
   * the real doc, its echo is redundant. Purely an id match — timing, ordering
   * and the grace window play no part.
   */
  ackFromFrame(chatId: string, messageIds: Iterable<string>): void {
    const sends = this.forChat(chatId);
    if (sends.length === 0) {
      return;
    }
    const confirmed = messageIds instanceof Set ? messageIds : new Set(messageIds);
    const next = sends.filter((send) => !confirmed.has(send.messageId));
    if (next.length === sends.length) {
      return;
    }
    if (next.length === 0) {
      this.#byChat.delete(chatId);
    } else {
      this.#byChat.set(chatId, next);
    }
    this.#emit();
  }

  /**
   * `retry_pending_send`: a retry is a NEW send of the same text, not a resend
   * of the old wire message — it mints a fresh id and restarts the grace-window
   * clock. The old pending send is swapped out in place so the bubble stays put.
   * Returns the new `PendingSend` (the caller ships it), or null if the id is
   * already gone (it was acked while the user was reaching for the button).
   */
  retry(
    messageId: string,
    options: { mintMessageId?: () => string; nowMs?: number } = {},
  ): PendingSend | null {
    const previous = this.get(messageId);
    if (previous === null) {
      return null;
    }
    const next: PendingSend = {
      ...previous,
      messageId: (options.mintMessageId ?? mintId)(),
      startedAtMs: options.nowMs ?? Date.now(),
    };
    const sends = this.forChat(previous.chatId);
    this.#byChat.set(
      previous.chatId,
      sends.map((send) => (send.messageId === messageId ? next : send)),
    );
    this.#emit();
    return next;
  }

  /**
   * `retry_pending_send` (state.rs:1235), the trailer-retry half: restart the
   * grace clock for EVERY pending send in the chat — same message ids, so the
   * overlay returns to its Sending phase while the engine's re-delivery runs.
   * (The durable re-issue itself is the `RETRY_DELIVERY` RPC the caller
   * fires; this never mints new ids.)
   */
  restartGrace(chatId: string, nowMs: number): void {
    const sends = this.forChat(chatId);
    if (sends.length === 0) {
      return;
    }
    this.#byChat.set(
      chatId,
      sends.map((send) => ({ ...send, startedAtMs: nowMs })),
    );
    this.#emit();
  }

  /** Test seam — drops every overlay without notifying anything of substance. */
  reset(): void {
    this.#byChat = new Map();
    this.#emit();
  }

  #emit(): void {
    for (const listener of this.#listeners) {
      listener();
    }
  }
}

export const echoStore = new EchoStore();

/**
 * Per-chat viewport memory (transcript.rs:2401 `saved_viewports`), module
 * scoped like the echo store: a `TranscriptStore` is created per open chat
 * and disposed on every switch, so the viewports must outlive them.
 */
export const savedViewportCache = new SavedViewportCache();

/**
 * Locally-authored queue rows whose stable ids have not appeared in the
 * transcript yet (transcript.rs:2307) — module scoped for the same reason as
 * the viewport cache. The web composer has no queue-send path yet, so nothing
 * registers today; the class ships tested for the composer ticket that adds
 * one.
 */
export const pendingQueuedTurns = new PendingQueuedTurns();

/** The message ids a frame carries — the ack key set. */
function frameMessageIds(frame: TranscriptFrame): string[] {
  if ("reset" in frame) {
    return frame.reset.map((entry) => entry.id);
  }
  return [...frame.upsert.map((item) => item.entry.id), ...frame.append.map((item) => item.entry)];
}

/** The watch surface the store needs — `EngineClient` satisfies it. */
export interface TranscriptClient {
  watch<T>(method: string, params: unknown, handlers: {
    onItem: (item: T, context: { generation: number }) => void;
    onEnd?: (error: RpcError | undefined) => void;
  }): WatchHandle;
}

/**
 * The offline transcript cache (§2.3, ticket 31): last-seen entries for
 * chats the user has actually opened, seeded before the live stream's
 * first frame and re-saved (debounced) as frames land. One entry per
 * `(engine, chat)` — the desktop's `chat-<sha256>.json` granularity.
 *
 * Ticket 81 — the seed carries WHEN it was saved. A seconds-old save is a
 * LIVE mid-run snapshot (the run is still streaming; the seed keeps its
 * `status: "streaming"` so the tail group renders open immediately — the
 * desktop's state-preserved switch, which reads the live doc). An old or
 * unstamped save is a DEAD session's leftover: its stale `streaming`
 * status is downgraded (ticket 80) so interrupted history renders closed.
 */
export interface TranscriptSeed {
  readonly entries: readonly SessionMessageEntry[];
  /** Epoch ms of the save; 0 = unknown (treated as a dead session's). */
  readonly savedAtMs: number;
}

export interface TranscriptCache {
  load(): Promise<TranscriptSeed | null>;
  save(entries: readonly SessionMessageEntry[]): Promise<void>;
}

/** Debounce window for cache writes — a burst of frames is one save. */
const CACHE_SAVE_DEBOUNCE_MS = 300;

/**
 * Ticket 81 — how old a cache save may be and still count as a live
 * mid-run snapshot. Streaming runs write frames continuously (the save
 * debounces at 300 ms), so live-switch seeds are seconds old; a silent
 * stretch longer than this (a very long tool call with no output)
 * misclassifies as stale and costs one closed→open transition on the next
 * switch — the live reset corrects it within its normal roundtrip.
 */
export const LIVE_SEED_STREAMING_MS = 300_000;

export class TranscriptStore {
  readonly #client: TranscriptClient;
  readonly #docId: string;
  readonly #log: (message: string, detail?: unknown) => void;
  readonly #echoes: EchoStore;
  readonly #cache: TranscriptCache | undefined;
  #saveTimer: ReturnType<typeof setTimeout> | undefined;
  #entries: readonly SessionMessageEntry[] = EMPTY_ENTRIES;
  #contextUsage: ContextUsage | null = null;
  #loaded = false;
  #error: string | null = null;
  #generation = 0;
  #replay: TranscriptReplayState = "pending";
  #baselineEpoch = 0;
  #baseline: TranscriptBaseline | null = null;
  #snapshot: TranscriptSnapshot;
  #handle: WatchHandle | null = null;
  readonly #listeners = new Set<() => void>();
  #disposed = false;

  constructor(
    client: EngineClient | TranscriptClient,
    docId: string,
    options: {
      log?: (message: string, detail?: unknown) => void;
      echoes?: EchoStore;
      /**
       * `Transcript::for_doc(follow)`: false mounts the store WITHOUT the
       * live watch — a frozen subagent snapshot's shape. The host seeds the
       * snapshot through `seedEntries` and falls back to the live doc watch
       * with `resubscribe()` when the blob fetch fails.
       */
      follow?: boolean;
      /** The offline cache — seeded pre-frame, saved debounced per frame. */
      cache?: TranscriptCache;
    } = {},
  ) {
    this.#client = client;
    this.#docId = docId;
    this.#log = options.log ?? (() => {});
    this.#echoes = options.echoes ?? echoStore;
    this.#cache = options.cache;
    this.#snapshot = this.#takeSnapshot();
    if (this.#cache !== undefined) {
      // Seed the last-seen entries while the live stream is still arriving;
      // the first live frame's reset replaces them wholesale. Ticket 81: a
      // seconds-old save is a LIVE mid-run snapshot — kept verbatim so the
      // tail group renders open immediately (the desktop's state-preserved
      // switch reads the live doc); an old or unstamped save is a dead
      // session's leftover, downgraded (ticket 80) so interrupted history
      // renders closed.
      void this.#cache
        .load()
        .then((seed) => {
          if (this.#disposed || this.#loaded || seed === null) {
            return;
          }
          const live = seed.savedAtMs > 0 && Date.now() - seed.savedAtMs <= LIVE_SEED_STREAMING_MS;
          this.seedEntries(live ? seed.entries : seed.entries.map(downgradeStaleStreaming));
        })
        .catch(() => {});
    }
    if (options.follow !== false) {
      this.#subscribe();
    }
  }

  /** The doc this store watches (a chat id, or a subagent doc id). */
  get docId(): string {
    return this.#docId;
  }

  getSnapshot(): TranscriptSnapshot {
    return this.#snapshot;
  }

  subscribe(listener: () => void): () => void {
    this.#listeners.add(listener);
    return () => {
      this.#listeners.delete(listener);
    };
  }

  /**
   * Drop the stream and re-subscribe for a fresh reset — the desync recovery
   * and the Retry affordance. The replay state follows the stream (it returns
   * to `"pending"` so the first populated frame re-arms the reveal baseline),
   * but the ROWS stay: the engine re-sends a full reset on re-subscribe and
   * its `preserveIdentity` swap lands atomically, so the surface never
   * observes an empty, unloaded transcript mid-session — rows are only
   * empty on a genuine chat switch (the desktop clears its rows exactly
   * there, transcript.rs:3974-3989, and re-derives them atomically from the
   * doc state, :4032-4057).
   */
  resubscribe(): void {
    if (this.#disposed) {
      return;
    }
    this.#handle?.cancel();
    this.#handle = null;
    this.#error = null;
    this.#replay = "pending";
    this.#subscribe();
    this.#commit();
  }

  /**
   * `set_subagent_snapshot` (state.rs, shell.rs:2778): seed a frozen
   * subagent's snapshot entries as the store's whole transcript — loaded,
   * settled, no watch. Only meaningful on a `follow: false` store.
   */
  seedEntries(entries: readonly SessionMessageEntry[]): void {
    if (this.#disposed) {
      return;
    }
    this.#entries = [...entries];
    this.#loaded = true;
    this.#error = null;
    this.#replay = "populated";
    // The cache seed IS a baseline (cached history must not animate), but a
    // distinguishable one: the live stream's first accepted reset publishes
    // the next epoch, so an authoritative reset is never mistaken for a
    // continuation of the cache.
    this.#publishBaseline("seed");
    this.#commit();
  }

  dispose(): void {
    if (this.#disposed) {
      return;
    }
    this.#disposed = true;
    if (this.#saveTimer !== undefined) {
      clearTimeout(this.#saveTimer);
      this.#saveTimer = undefined;
    }
    this.#handle?.cancel();
    this.#handle = null;
    this.#listeners.clear();
  }

  #subscribe(): void {
    this.#handle = this.#client.watch<TranscriptUpdate>(methods.WATCH_DOC_MESSAGES, { chatId: this.#docId }, {
      onItem: (item, { generation }) => this.#onItem(item, generation),
      onEnd: (error) => this.#onEnd(error),
    });
  }

  #onItem(update: TranscriptUpdate, generation: number): void {
    if (this.#disposed || generation < this.#generation) {
      // A stale stream from before a reconnect must not re-apply old frames.
      return;
    }
    if (generation > this.#generation) {
      // New connection generation: the stream re-sends a full reset first,
      // so adopting it here only guards a misbehaving peer. The rows and the
      // loaded flag stay — the reset replaces the entries wholesale
      // (`preserveIdentity`) and rows are never observed empty mid-session.
      // The pending replay is committed BEFORE the frame applies, so the
      // surface observes it (a reconnect's reset is a replay baseline, like
      // the desktop's re-attach) instead of the swap and the reset landing
      // as one populated commit.
      this.#generation = generation;
      this.#error = null;
      this.#replay = "pending";
      this.#commit();
    }
    const frame = asFrame(update);
    if (frame === null) {
      this.#log("dropped malformed transcript frame", update);
      return;
    }
    try {
      this.#entries = applyTranscriptFrame(this.#entries, frame);
    } catch (error) {
      if (error instanceof TranscriptDesync) {
        this.#log("transcript desync; resubscribing for a reset", error.message);
        this.resubscribe();
        return;
      }
      throw error;
    }
    // `ack_pending_send_from_transcript`: any echo whose id the host has now
    // written back is redundant, so it goes on the same frame that confirms it
    // — otherwise the bubble would double for a tick.
    this.#echoes.ackFromFrame(this.#docId, frameMessageIds(frame));
    if (update.contextUsage !== undefined) {
      this.#contextUsage = update.contextUsage;
    }
    this.#loaded = true;
    // The replay state: a reset decides authoritatively (empty vs populated);
    // any delta means real rows exist.
    if ("reset" in frame) {
      this.#replay = frame.reset.length === 0 ? "empty" : "populated";
      // The ACCEPTED reset is the replay baseline (ticket 69) — published only
      // here, after the frame applied, so stale/malformed frames and bare
      // resubscribe windows never advance it. An authoritative empty reset is
      // a baseline too (empty stays authoritative).
      this.#publishBaseline("reset");
    } else {
      this.#replay = "populated";
    }
    this.#commit();
  }

  #onEnd(error: RpcError | undefined): void {
    if (this.#disposed || error === undefined) {
      return;
    }
    this.#error = error.message;
    this.#commit();
  }

  #publishBaseline(provenance: TranscriptBaseline["provenance"]): void {
    this.#baselineEpoch += 1;
    this.#baseline = { epoch: this.#baselineEpoch, provenance, entries: this.#entries };
  }

  #takeSnapshot(): TranscriptSnapshot {
    const last = this.#entries[this.#entries.length - 1];
    return {
      entries: this.#entries,
      contextUsage: this.#contextUsage,
      loaded: this.#loaded,
      streaming: last?.status === "streaming",
      error: this.#error,
      generation: this.#generation,
      replay: this.#replay,
      baseline: this.#baseline,
    };
  }

  #commit(): void {
    this.#snapshot = this.#takeSnapshot();
    this.#scheduleCacheSave();
    for (const listener of this.#listeners) {
      listener();
    }
  }

  /** Debounced cache write — only for stores that actually loaded rows. */
  #scheduleCacheSave(): void {
    if (this.#cache === undefined || this.#saveTimer !== undefined || !this.#loaded) {
      return;
    }
    this.#saveTimer = setTimeout(() => {
      this.#saveTimer = undefined;
      if (this.#disposed || this.#cache === undefined || !this.#loaded) {
        return;
      }
      void this.#cache.save(this.#entries).catch(() => {});
    }, CACHE_SAVE_DEBOUNCE_MS);
  }
}

/** Split a `TranscriptUpdate` into its frame, tolerating malformed items. */
function asFrame(update: TranscriptUpdate): TranscriptFrame | null {
  if (typeof update !== "object" || update === null) {
    return null;
  }
  if ("reset" in update) {
    return Array.isArray(update.reset) ? { reset: update.reset } : null;
  }
  const delta = update as { upsert?: unknown; append?: unknown; remove?: unknown; count?: unknown };
  if (typeof delta.count !== "number") {
    return null;
  }
  return {
    upsert: Array.isArray(delta.upsert) ? delta.upsert : [],
    append: Array.isArray(delta.append) ? delta.append : [],
    remove: Array.isArray(delta.remove) ? delta.remove : [],
    count: delta.count,
  };
}

/**
 * Ticket 80 — the offline cache saves raw entries, so a chat left mid-run
 * seeds with `status: "streaming"`. A previous session's save cannot still
 * be streaming: the run was interrupted when the app closed, and the stale
 * status would render the last tool group auto-opened, then visibly close
 * when the authoritative reset settles it — the "old tool calls opening"
 * replay on the new-chat → chat route. Downgrade at SEED time only
 * (presentation): the cache keeps saving raw entries, subagent-snapshot
 * `seedEntries` callers are untouched (their data is fresh from a live
 * engine), and the live reset reports the engine's truth moments later.
 */
function downgradeStaleStreaming(entry: SessionMessageEntry): SessionMessageEntry {
  return entry.status === "streaming" ? { ...entry, status: "aborted" } : entry;
}
