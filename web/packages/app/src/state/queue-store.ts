import type {
  BeginQueueEditOutcome,
  FinishQueueEditOutcome,
  QueueDeliveryGate,
  QueuedMessage,
  RenewQueueEditOutcome,
  WatchQueueSnapshot,
} from "@zeron/proto";
import type { EngineClient, WatchHandle } from "@zeron/engine-client";
import { methods, RpcError } from "@zeron/engine-client";
import {
  moveQueuedMessage as moveQueuedMessageRpc,
  removeQueuedMessage as removeQueuedMessageRpc,
  sendQueuedMessageNow as sendQueuedMessageNowRpc,
  steerQueuedMessageNow as steerQueuedMessageNowRpc,
} from "../lib/queue-actions";

/**
 * The per-chat queue store — web peer of `crates/ui/src/queue.rs` + the
 * engine's `WatchQueue` snapshot stream. One store per open chat page;
 * the chat page owns its lifecycle and disposes it on chat switch.
 *
 * The store keeps three pieces of state on top of the raw rows:
 *
 * - the **rows** the engine last emitted, identity-stable per id (so
 *   row-level memoization holds; the watch stream is snapshot-then-snapshot
 *   so a fresh `items` array lands in full each time);
 * - a **local edit lease** for the row the user is currently editing in
 *   this session — the host's authoritative `Editing` gate may out-live our
 *   view of the row, so the store tracks its own lease id + expiry and
 *   surfaces that separately from the engine's gate;
 * - the **error** of the current stream subscription (degraded to null
 *   once a fresh item arrives; the chat page may retry via `resubscribe`).
 *
 * Same React-binding contract as the watch cache / transcript store:
 * `getSnapshot()` is identity-stable until an actual change, `subscribe`
 * fires once per change.
 */

export interface QueueSnapshot {
  readonly rows: readonly QueuedMessage[];
  readonly loaded: boolean;
  readonly error: string | null;
  /** Generation of the most recent item applied (for React binding). */
  readonly generation: number;
  /** The locally-held edit lease, if any — mirrors the host gate but
   *  survives a row mutation the host has not yet echoed back. */
  readonly editLease: QueueEditLeaseState | null;
}

export interface QueueEditLeaseState {
  readonly messageId: string;
  readonly leaseId: string;
  readonly baseText: string;
  readonly baseTextHash: string;
  /** Epoch millis the lease lapses; renewal must beat this. */
  readonly expiresAtMs: number;
  /** The host device id we acquired against. */
  readonly ownerDeviceId: string;
  /** The instance id we minted for this edit session. */
  readonly editorInstanceId: string;
}

/** Test seams — the bare shape `EngineClient` matches. */
export interface QueueClient {
  call<T>(method: string, params?: unknown): Promise<T>;
  watch<T>(method: string, params: unknown, handlers: {
    onItem: (item: T, context: { generation: number }) => void;
    onEnd?: (error: RpcError | undefined) => void;
  }): WatchHandle;
}

/**
 * Begin-edit reply normalized to the lease fields the store records. The
 * `acquired` arm carries the row's committed attachment paths so the caller
 * can stage them into the composer (queue.rs::begin_queue_edit's loaded
 * attachments, ticket 16).
 */
export type BeginLeaseOutcome =
  | {
      kind: "acquired";
      leaseId: string;
      text: string;
      baseTextHash: string;
      expiresAtMs: number;
      attachments: readonly string[];
    }
  | { kind: "locked"; ownerDeviceId: string; expiresAtMs: number }
  | { kind: "missing" };

/** Finish-edit reply narrowed to the outcomes the UI handles. */
export type FinishLeaseOutcome =
  | { kind: "committed" }
  | { kind: "cancelled" }
  | { kind: "discarded" }
  | { kind: "released" }
  | { kind: "conflict"; currentText: string }
  | { kind: "lost" }
  | { kind: "missing" };

export interface QueueStoreOptions {
  readonly log?: (message: string, detail?: unknown) => void;
  readonly editorDeviceId: string;
}

const EMPTY_ROWS: readonly QueuedMessage[] = [];

function beginOutcome(outcome: BeginQueueEditOutcome): BeginLeaseOutcome {
  switch (outcome.outcome) {
    case "acquired":
      return {
        kind: "acquired",
        leaseId: outcome.leaseId,
        text: outcome.text,
        baseTextHash: outcome.baseTextHash,
        expiresAtMs: outcome.expiresAtMs,
        attachments: Array.isArray(outcome.attachments) ? outcome.attachments : [],
      };
    case "locked":
      return { kind: "locked", ownerDeviceId: outcome.ownerDeviceId, expiresAtMs: outcome.expiresAtMs };
    case "missing":
      return { kind: "missing" };
  }
}

function finishOutcome(outcome: FinishQueueEditOutcome): FinishLeaseOutcome {
  switch (outcome.outcome) {
    case "committed":
      return { kind: "committed" };
    case "cancelled":
      return { kind: "cancelled" };
    case "discarded":
      return { kind: "discarded" };
    case "released":
      return { kind: "released" };
    case "conflict":
      return { kind: "conflict", currentText: outcome.currentText };
    case "lost":
      return { kind: "lost" };
    case "missing":
      return { kind: "missing" };
  }
}

function renewOutcome(outcome: RenewQueueEditOutcome): { kind: "renewed"; expiresAtMs: number } | { kind: "lost" | "missing" } {
  switch (outcome.outcome) {
    case "renewed":
      return { kind: "renewed", expiresAtMs: outcome.expiresAtMs };
    case "lost":
      return { kind: "lost" };
    case "missing":
      return { kind: "missing" };
  }
}

export class QueueStore {
  readonly #client: QueueClient;
  readonly #chatId: string;
  readonly #log: (message: string, detail?: unknown) => void;
  readonly #editorDeviceId: string;
  #rows: readonly QueuedMessage[] = EMPTY_ROWS;
  #loaded = false;
  #error: string | null = null;
  #generation = 0;
  #editLease: QueueEditLeaseState | null = null;
  #handle: WatchHandle | null = null;
  #snapshot: QueueSnapshot;
  readonly #listeners = new Set<() => void>();
  #disposed = false;

  constructor(
    client: EngineClient | QueueClient,
    chatId: string,
    options: QueueStoreOptions,
  ) {
    this.#client = client;
    this.#chatId = chatId;
    this.#log = options.log ?? (() => {});
    this.#editorDeviceId = options.editorDeviceId;
    this.#snapshot = this.#takeSnapshot();
    this.#subscribe();
  }

  get chatId(): string {
    return this.#chatId;
  }

  getSnapshot(): QueueSnapshot {
    return this.#snapshot;
  }

  subscribe(listener: () => void): () => void {
    this.#listeners.add(listener);
    return () => {
      this.#listeners.delete(listener);
    };
  }

  /**
   * Drop the stream and re-subscribe for a fresh first item — the
   * recovery path when the queue view drifts from the host (e.g. an
   * edit-lease conflict that another device already resolved).
   */
  resubscribe(): void {
    if (this.#disposed) {
      return;
    }
    this.#handle?.cancel();
    this.#handle = null;
    this.#rows = EMPTY_ROWS;
    this.#loaded = false;
    this.#error = null;
    this.#subscribe();
    this.#commit();
  }

  dispose(): void {
    if (this.#disposed) {
      return;
    }
    this.#disposed = true;
    this.#handle?.cancel();
    this.#handle = null;
    this.#editLease = null;
    this.#listeners.clear();
  }

  /** Begin editing — thin RPC wrapper that records the lease locally. */
  async beginEdit(messageId: string, editorInstanceId: string): Promise<BeginLeaseOutcome> {
    const outcome = beginOutcome(
      await this.#client.call<BeginQueueEditOutcome>(methods.BEGIN_QUEUED_MESSAGE_EDIT, {
        chatId: this.#chatId,
        id: messageId,
        editorDeviceId: this.#editorDeviceId,
        editorInstanceId,
      }),
    );
    if (outcome.kind === "acquired") {
      this.#editLease = {
        messageId,
        leaseId: outcome.leaseId,
        baseText: outcome.text,
        baseTextHash: outcome.baseTextHash,
        expiresAtMs: outcome.expiresAtMs,
        ownerDeviceId: this.#editorDeviceId,
        editorInstanceId,
      };
      this.#commit();
    }
    return outcome;
  }

  /** Renew the locally-held lease; a lost lease clears local state. */
  async renewEdit(): Promise<{ kind: "renewed"; expiresAtMs: number } | { kind: "lost" | "missing" }> {
    const lease = this.#editLease;
    if (lease === null) {
      return { kind: "missing" };
    }
    const result = renewOutcome(
      await this.#client.call<RenewQueueEditOutcome>(methods.RENEW_QUEUED_MESSAGE_EDIT, {
        chatId: this.#chatId,
        id: lease.messageId,
        leaseId: lease.leaseId,
      }),
    );
    if (result.kind === "renewed") {
      this.#editLease = { ...lease, expiresAtMs: result.expiresAtMs };
      this.#commit();
      return result;
    }
    this.#editLease = null;
    this.#commit();
    return result;
  }

  /** Finish the locally-held edit. Records the outcome and clears the lease. */
  async finishEdit(
    action: "commit" | "cancel" | "discard" | "releaseUnchanged",
    options: { text?: string; attachments?: readonly string[] } = {},
  ): Promise<FinishLeaseOutcome> {
    const lease = this.#editLease;
    if (lease === null) {
      return { kind: "missing" };
    }
    const params: Record<string, unknown> = {
      chatId: this.#chatId,
      id: lease.messageId,
      leaseId: lease.leaseId,
      action,
    };
    if (options.text !== undefined) {
      params["text"] = options.text;
      params["expectedTextHash"] = lease.baseTextHash;
    }
    if (options.attachments !== undefined && options.attachments.length > 0) {
      params["attachments"] = [...options.attachments];
    }
    const result = finishOutcome(
      await this.#client.call<FinishQueueEditOutcome>(methods.FINISH_QUEUED_MESSAGE_EDIT, params),
    );
    this.#editLease = null;
    this.#commit();
    return result;
  }

  /** Whether this store holds the active edit lease on `messageId`. */
  hasEditLease(messageId: string): boolean {
    const lease = this.#editLease;
    return lease !== null && lease.messageId === messageId;
  }

  // ── Bound mutations (chatId is implicit on this store) ───────────────

  /** Send a queued row now. Returns the `sent` ack (false = raced). */
  async sendNow(messageId: string): Promise<boolean> {
    return sendQueuedMessageNowRpc(this.#client, this.#chatId, messageId);
  }

  /**
   * Steer the live run with this queued row. Returns the `sent` ack.
   *
   * No UI call site, by design: the desktop's queue row offers Send now only
   * ("All providers use Send now", `queue.rs`) and spec decision #3 keeps
   * Steer-now off the web too. Kept available for a future capability.
   */
  async steerNow(messageId: string): Promise<boolean> {
    return steerQueuedMessageNowRpc(this.#client, this.#chatId, messageId);
  }

  /** Move a queued row to a new index. Returns the `changed` ack. */
  async move(messageId: string, toIndex: number): Promise<boolean> {
    return moveQueuedMessageRpc(this.#client, this.#chatId, messageId, toIndex);
  }

  /** Remove a queued row. Returns the `removed` ack. */
  async remove(messageId: string): Promise<boolean> {
    return removeQueuedMessageRpc(this.#client, this.#chatId, messageId);
  }

  /** The current row for an id, or null. */
  rowById(messageId: string): QueuedMessage | null {
    return this.#rows.find((row) => row.id === messageId) ?? null;
  }

  /** Whether the host gate (if any) on `messageId` is held by us. */
  isOurGate(messageId: string): boolean {
    const row = this.rowById(messageId);
    const gate = row?.deliveryGate ?? null;
    const lease = this.#editLease;
    if (lease === null || lease.messageId !== messageId) {
      return false;
    }
    if (gate === null) {
      return false;
    }
    if (gate.kind !== "editing") {
      return false;
    }
    return gate.leaseId === lease.leaseId;
  }

  #subscribe(): void {
    this.#handle = this.#client.watch<WatchQueueSnapshot | unknown>(
      methods.WATCH_QUEUE,
      { chatId: this.#chatId },
      {
        onItem: (item, ctx) => this.#onItem(item, ctx.generation),
        onEnd: (error) => this.#onEnd(error),
      },
    );
  }

  #onItem(item: unknown, generation: number): void {
    if (this.#disposed) {
      return;
    }
    if (generation < this.#generation) {
      return;
    }
    if (generation > this.#generation) {
      this.#generation = generation;
      this.#rows = EMPTY_ROWS;
      this.#loaded = false;
      this.#error = null;
    }
    const snapshot = asSnapshot(item);
    if (snapshot === null) {
      this.#log("queue: dropped malformed snapshot", item);
      return;
    }
    this.#rows = applyRows(this.#rows, snapshot.items);
    this.#loaded = true;
    this.#error = null;
    this.#commit();
  }

  #onEnd(error: RpcError | undefined): void {
    if (this.#disposed || error === undefined) {
      return;
    }
    this.#error = error.message;
    this.#commit();
  }

  #takeSnapshot(): QueueSnapshot {
    return {
      rows: this.#rows,
      loaded: this.#loaded,
      error: this.#error,
      generation: this.#generation,
      editLease: this.#editLease,
    };
  }

  #commit(): void {
    this.#snapshot = this.#takeSnapshot();
    for (const listener of this.#listeners) {
      try {
        listener();
      } catch (error) {
        this.#log("queue listener threw", describeError(error));
      }
    }
  }
}

/** Drop items that aren't shaped like `{items: QueuedMessage[]}`. */
function asSnapshot(value: unknown): WatchQueueSnapshot | null {
  if (typeof value !== "object" || value === null || Array.isArray(value)) {
    return null;
  }
  const items = (value as { items?: unknown }).items;
  if (!Array.isArray(items)) {
    return null;
  }
  const normalized: QueuedMessage[] = [];
  for (const item of items) {
    if (!isQueuedMessage(item)) {
      return null;
    }
    normalized.push(item);
  }
  return { items: normalized };
}

function isQueuedMessage(value: unknown): value is QueuedMessage {
  if (typeof value !== "object" || value === null) {
    return false;
  }
  const record = value as Record<string, unknown>;
  return typeof record["id"] === "string" && typeof record["text"] === "string" && typeof record["issuedBy"] === "string" && typeof record["issuedAt"] === "number";
}

/** Preserve object identity for unchanged rows so React memoization holds. */
function applyRows(current: readonly QueuedMessage[], incoming: readonly QueuedMessage[]): readonly QueuedMessage[] {
  const byId = new Map<string, QueuedMessage>();
  for (const row of current) {
    byId.set(row.id, row);
  }
  const next: QueuedMessage[] = [];
  for (const row of incoming) {
    const existing = byId.get(row.id);
    next.push(existing !== undefined && shallowSame(existing, row) ? existing : row);
  }
  return next;
}

/** Identity-stable comparison: same id + same wire payload means the row hasn't moved. */
function shallowSame(a: QueuedMessage, b: QueuedMessage): boolean {
  if (a.text !== b.text) {
    return false;
  }
  if (a.issuedBy !== b.issuedBy) {
    return false;
  }
  if (a.issuedAt !== b.issuedAt) {
    return false;
  }
  if ((a.editedAt ?? null) !== (b.editedAt ?? null)) {
    return false;
  }
  if ((a.holdForTurnEnd ?? false) !== (b.holdForTurnEnd ?? false)) {
    return false;
  }
  return sameGate(a.deliveryGate ?? null, b.deliveryGate ?? null);
}

function sameGate(a: QueueDeliveryGate | null, b: QueueDeliveryGate | null): boolean {
  if (a === null && b === null) {
    return true;
  }
  if (a === null || b === null) {
    return false;
  }
  if (a.kind !== b.kind) {
    return false;
  }
  if (a.kind === "editing" && b.kind === "editing") {
    return (
      a.leaseId === b.leaseId &&
      a.ownerDeviceId === b.ownerDeviceId &&
      a.expiresAtMs === b.expiresAtMs &&
      a.baseTextHash === b.baseTextHash
    );
  }
  if (a.kind === "reviewRequired" && b.kind === "reviewRequired") {
    return (
      a.previousLeaseId === b.previousLeaseId &&
      a.ownerDeviceId === b.ownerDeviceId &&
      a.sinceMs === b.sinceMs &&
      a.baseTextHash === b.baseTextHash
    );
  }
  return false;
}

function describeError(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}