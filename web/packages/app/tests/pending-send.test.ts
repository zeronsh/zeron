import { describe, expect, it, vi } from "vitest";
import type { SessionMessageEntry, TranscriptUpdate } from "@zeron/proto";
import {
  EchoStore,
  TranscriptStore,
  UNDELIVERED_GRACE_MS,
  chatDeliveryDegraded,
  pendingSendStatus,
  transcriptSnapshotIsLive,
  type PendingSend,
  type TranscriptCache,
  type TranscriptClient,
  type TranscriptSeed,
} from "../src/state/transcript-store";
import { sendRun, type DraftConfig } from "../src/lib/composer-actions";

/**
 * Ports of the desktop's echo / pending-send tests (`crates/ui/src/state.rs`,
 * research 14-state-behavior §4.3), plus the web-only "one id per send"
 * invariant that makes the ack possible at all.
 */

const CHAT = "chat-1";

function pending(overrides: Partial<PendingSend> = {}): PendingSend {
  return {
    messageId: "msg-1",
    chatId: CHAT,
    startedAtMs: 1_000,
    text: "hello",
    attachmentPaths: [],
    ...overrides,
  };
}

function userEntry(id: string): SessionMessageEntry {
  return {
    id,
    role: "user",
    parts: [{ kind: "text", id: `${id}#0`, text: "hello" }],
    createdAt: 1_000,
    deviceId: "device-1",
  };
}

describe("echo overlay", () => {
  it("echoes_show_until_doc_frame_confirms", () => {
    const store = new EchoStore();
    store.pushEcho(pending());
    expect(store.forChat(CHAT)).toHaveLength(1);

    // A frame that does NOT name the id leaves the echo alone.
    store.ackFromFrame(CHAT, ["someone-else"]);
    expect(store.forChat(CHAT)).toHaveLength(1);

    store.ackFromFrame(CHAT, ["msg-1"]);
    expect(store.forChat(CHAT)).toHaveLength(0);
  });

  it("send_pending_overlays_working_until_the_grace_window", () => {
    const send = pending({ startedAtMs: 0 });
    expect(pendingSendStatus(send, 0)).toBe("pending");
    expect(pendingSendStatus(send, UNDELIVERED_GRACE_MS)).toBe("pending");
    expect(pendingSendStatus(send, UNDELIVERED_GRACE_MS + 1)).toBe("undelivered");
    // The AND-in point for a future `chat_delivery_degraded`: degraded
    // delivery keeps the send quiet however long it has been waiting.
    expect(pendingSendStatus(send, UNDELIVERED_GRACE_MS + 1, true)).toBe("pending");
  });

  it("chatDeliveryDegraded maps the routed engine's connectivity posture (state.rs:877-891)", () => {
    // The desktop's `Disabled => false` early return and the offline/
    // reconnecting degradation arms; an unobserved slot stays quiet.
    expect(chatDeliveryDegraded("offline")).toBe(true);
    expect(chatDeliveryDegraded("reconnecting")).toBe(true);
    expect(chatDeliveryDegraded("connected")).toBe(false);
    expect(chatDeliveryDegraded("disabled")).toBe(false);
    expect(chatDeliveryDegraded(null)).toBe(false);
    expect(chatDeliveryDegraded(undefined)).toBe(false);
  });

  it("a_degraded_engine_never_fabricates_the_undelivered_state", () => {
    // The false-"Not delivered" retry the spec finding names: an outage
    // the engine itself reported (fake connectivity value → the web arm)
    // holds the send pending past the grace window, and the honest state
    // returns the moment the path heals.
    const send = pending({ startedAtMs: 0 });
    const at = UNDELIVERED_GRACE_MS + 1;
    expect(pendingSendStatus(send, at, chatDeliveryDegraded("offline"))).toBe("pending");
    expect(pendingSendStatus(send, at, chatDeliveryDegraded("reconnecting"))).toBe("pending");
    expect(pendingSendStatus(send, at, chatDeliveryDegraded("connected"))).toBe("undelivered");
    expect(pendingSendStatus(send, at, chatDeliveryDegraded("disabled"))).toBe("undelivered");
  });

  it("send_pending_acked_when_the_host_writes_the_message_back", () => {
    const store = new EchoStore();
    // Well past the grace window — acking is purely an id match, and timing
    // plays no part in it.
    store.pushEcho(pending({ startedAtMs: Date.now() - UNDELIVERED_GRACE_MS * 10 }));
    store.ackFromFrame(CHAT, new Set(["msg-1"]));
    expect(store.forChat(CHAT)).toHaveLength(0);
  });

  it("send_failure_cleanup_only_ends_its_own_overlay", () => {
    const store = new EchoStore();
    store.pushEcho(pending({ messageId: "msg-1" }));
    store.pushEcho(pending({ messageId: "msg-2", text: "second" }));
    expect(store.forChat(CHAT)).toHaveLength(2);

    store.removeEcho("msg-1");
    const left = store.forChat(CHAT);
    expect(left).toHaveLength(1);
    expect(left[0]?.messageId).toBe("msg-2");
    expect(left[0]?.text).toBe("second");
  });

  it("retry_restarts_the_grace_window_with_a_new_id", () => {
    const store = new EchoStore();
    store.pushEcho(pending({ startedAtMs: 0 }));
    expect(pendingSendStatus(store.forChat(CHAT)[0]!, UNDELIVERED_GRACE_MS + 1)).toBe("undelivered");

    const next = store.retry("msg-1", { mintMessageId: () => "msg-2", nowMs: 500_000 });
    expect(next).not.toBeNull();
    expect(next!.messageId).toBe("msg-2");
    expect(next!.startedAtMs).toBe(500_000);
    expect(next!.text).toBe("hello");
    // Swapped IN PLACE: one bubble before, one bubble after.
    expect(store.forChat(CHAT)).toHaveLength(1);
    expect(store.get("msg-1")).toBeNull();
    // And the clock restarted: it is quiet again at the same wall time.
    expect(pendingSendStatus(next!, 500_000 + 1_000)).toBe("pending");
  });

  it("retrying an already-acked send is a no-op", () => {
    const store = new EchoStore();
    expect(store.retry("gone")).toBeNull();
  });

  it("a push of an id already on the overlay does not double the bubble", () => {
    const store = new EchoStore();
    store.pushEcho(pending());
    store.pushEcho(pending({ text: "different text, same id" }));
    expect(store.forChat(CHAT)).toHaveLength(1);
    expect(store.forChat(CHAT)[0]?.text).toBe("hello");
  });

  it("acks are chat-scoped: a frame on one chat never clears another's echo", () => {
    const store = new EchoStore();
    store.pushEcho(pending({ messageId: "msg-1", chatId: "chat-a" }));
    store.pushEcho(pending({ messageId: "msg-1", chatId: "chat-b" }));
    store.ackFromFrame("chat-a", ["msg-1"]);
    expect(store.forChat("chat-a")).toHaveLength(0);
    expect(store.forChat("chat-b")).toHaveLength(1);
  });
});

describe("TranscriptStore acks the overlay from the stream", () => {
  function fakeClient(): {
    client: TranscriptClient;
    emit: (update: TranscriptUpdate, generation?: number) => void;
  } {
    let onItem: ((item: TranscriptUpdate, ctx: { generation: number }) => void) | null = null;
    return {
      client: {
        watch<T>(
          _method: string,
          _params: unknown,
          handlers: { onItem: (item: T, ctx: { generation: number }) => void },
        ) {
          onItem = handlers.onItem as (item: TranscriptUpdate, ctx: { generation: number }) => void;
          return { cancel: () => {} };
        },
      } as TranscriptClient,
      emit: (update, generation = 1) => onItem?.(update, { generation }),
    };
  }

  it("a reset frame carrying the id clears the echo", () => {
    const echoes = new EchoStore();
    echoes.pushEcho(pending());
    const { client, emit } = fakeClient();
    const store = new TranscriptStore(client, CHAT, { echoes });

    emit({ contextUsage: null, reset: [userEntry("msg-1")] });
    expect(echoes.forChat(CHAT)).toHaveLength(0);
    expect(store.getSnapshot().entries).toHaveLength(1);
    store.dispose();
  });

  it("a delta upsert carrying the id clears the echo", () => {
    const echoes = new EchoStore();
    echoes.pushEcho(pending());
    const { client, emit } = fakeClient();
    const store = new TranscriptStore(client, CHAT, { echoes });

    emit({ contextUsage: null, reset: [] });
    emit({
      contextUsage: null,
      upsert: [{ after: null, entry: userEntry("msg-1") }],
      append: [],
      remove: [],
      count: 1,
    });
    expect(echoes.forChat(CHAT)).toHaveLength(0);
    store.dispose();
  });
});

describe("TranscriptStore reset without the empty window (ticket 40)", () => {
  function fakeClient(): {
    client: TranscriptClient;
    emit: (update: TranscriptUpdate, generation?: number) => void;
  } {
    let onItem: ((item: TranscriptUpdate, ctx: { generation: number }) => void) | null = null;
    return {
      client: {
        watch<T>(
          _method: string,
          _params: unknown,
          handlers: { onItem: (item: T, ctx: { generation: number }) => void },
        ) {
          onItem = handlers.onItem as (item: TranscriptUpdate, ctx: { generation: number }) => void;
          return { cancel: () => {} };
        },
      } as TranscriptClient,
      emit: (update, generation = 1) => onItem?.(update, { generation }),
    };
  }

  /** A delta that cannot apply — the desync tripwire (resubscribe path). */
  const DESYNC_DELTA: TranscriptUpdate = {
    contextUsage: null,
    upsert: [],
    append: [{ entry: "ghost", part: "p0", text: "x", len: 1 }],
    remove: [],
    count: 3,
  };

  it("a_desync_resubscribe_keeps_the_previous_entries_until_the_reset_frame_lands", () => {
    const { client, emit } = fakeClient();
    const store = new TranscriptStore(client, CHAT, { echoes: new EchoStore() });
    const a = userEntry("a");
    const b = userEntry("b");
    emit({ contextUsage: null, reset: [a, b] });
    const settled = store.getSnapshot();
    expect(settled.entries).toHaveLength(2);
    expect(settled.loaded).toBe(true);
    expect(settled.replay).toBe("populated");

    // The desync trips the resubscribe: the rows STAY (rows are only empty
    // on a genuine chat switch — the desktop re-derives them atomically,
    // transcript.rs:4032-4057); only the replay returns to "pending".
    emit(DESYNC_DELTA);
    const mid = store.getSnapshot();
    expect(mid.entries).toBe(settled.entries);
    expect(mid.loaded).toBe(true);
    expect(mid.replay).toBe("pending");

    // The fresh stream's reset lands as an atomic swap: deep-equal entries
    // keep their object identities (`preserveIdentity`), so the row cache
    // and the measured-height map survive it.
    emit({ contextUsage: null, reset: [userEntry("a"), userEntry("b")] });
    const after = store.getSnapshot();
    expect(after.replay).toBe("populated");
    expect(after.entries[0]).toBe(a);
    expect(after.entries[1]).toBe(b);
    store.dispose();
  });

  it("a_generation_swap_keeps_the_previous_entries_until_the_reset_frame_lands", () => {
    const { client, emit } = fakeClient();
    const store = new TranscriptStore(client, CHAT, { echoes: new EchoStore() });
    const a = userEntry("a");
    emit({ contextUsage: null, reset: [a] });
    expect(store.getSnapshot().replay).toBe("populated");

    // A reconnect bumps the generation: the swap commits the pending window
    // BEFORE the frame applies (the surface observes it, so the coming reset
    // re-arms the reveal baseline), and the rows stay put throughout — an
    // empty, unloaded snapshot is never published.
    const observed: { replay: string; entries: number }[] = [];
    const unsubscribe = store.subscribe(() => {
      const snap = store.getSnapshot();
      observed.push({ replay: snap.replay, entries: snap.entries.length });
    });
    emit({ contextUsage: null, upsert: [], append: [], remove: [], count: 1 }, 2);
    unsubscribe();
    expect(observed.map((o) => o.replay)).toEqual(["pending", "populated"]);
    expect(observed.every((o) => o.entries === 1)).toBe(true);

    // The stale-stream guard stays: a frame from the OLD generation is
    // dropped without touching the snapshot.
    emit({ contextUsage: null, reset: [userEntry("zzz")] }, 1);
    expect(store.getSnapshot().entries).toHaveLength(1);
    expect(store.getSnapshot().replay).toBe("populated");

    // The new stream's reset lands: populated, identity preserved.
    emit({ contextUsage: null, reset: [userEntry("a")] }, 2);
    const after = store.getSnapshot();
    expect(after.replay).toBe("populated");
    expect(after.entries[0]).toBe(a);
    store.dispose();
  });
});

describe("TranscriptStore accepted-reset baseline epochs (ticket 69)", () => {
  it("arrival waits for a live reset even when a seed is loaded (ticket 82)", () => {
    const { client, emit } = fakeClient();
    const store = new TranscriptStore(client, CHAT, { echoes: new EchoStore() });
    expect(transcriptSnapshotIsLive(store.getSnapshot())).toBe(false);

    store.seedEntries([userEntry("cached")]);
    expect(store.getSnapshot().loaded).toBe(true);
    expect(transcriptSnapshotIsLive(store.getSnapshot())).toBe(false);

    emit({ contextUsage: null, reset: [userEntry("live")] });
    expect(transcriptSnapshotIsLive(store.getSnapshot())).toBe(true);
    // Empty authoritative history also releases the gate.
    emit({ contextUsage: null, reset: [] });
    expect(transcriptSnapshotIsLive(store.getSnapshot())).toBe(true);
    store.dispose();
  });

  it("a terminal error releases cached or unloaded content (ticket 82)", () => {
    const { client } = fakeClient();
    const store = new TranscriptStore(client, CHAT, { echoes: new EchoStore() });
    expect(transcriptSnapshotIsLive({ ...store.getSnapshot(), error: "offline" })).toBe(true);
    store.seedEntries([userEntry("cached")]);
    expect(transcriptSnapshotIsLive({ ...store.getSnapshot(), error: "offline" })).toBe(true);
    // The contract is non-null, even if the engine supplies an empty message.
    expect(transcriptSnapshotIsLive({ ...store.getSnapshot(), error: "" })).toBe(true);
    store.dispose();
  });

  function fakeClient(): {
    client: TranscriptClient;
    emit: (update: TranscriptUpdate, generation?: number) => void;
  } {
    let onItem: ((item: TranscriptUpdate, ctx: { generation: number }) => void) | null = null;
    return {
      client: {
        watch<T>(
          _method: string,
          _params: unknown,
          handlers: { onItem: (item: T, ctx: { generation: number }) => void },
        ) {
          onItem = handlers.onItem as (item: TranscriptUpdate, ctx: { generation: number }) => void;
          return { cancel: () => {} };
        },
      } as TranscriptClient,
      emit: (update, generation = 1) => onItem?.(update, { generation }),
    };
  }

  function deferredCache(): { cache: TranscriptCache; resolve: (seed: TranscriptSeed | null) => void } {
    let resolve!: (seed: TranscriptSeed | null) => void;
    const promise = new Promise<TranscriptSeed | null>((res) => {
      resolve = res;
    });
    return { cache: { load: () => promise, save: () => Promise.resolve() }, resolve };
  }

  it("accepted resets advance the epoch; stale and malformed frames do not", () => {
    const { client, emit } = fakeClient();
    const store = new TranscriptStore(client, CHAT, { echoes: new EchoStore() });
    expect(store.getSnapshot().baseline).toBeNull();

    // The first accepted reset publishes the first baseline.
    emit({ contextUsage: null, reset: [userEntry("a")] }, 1);
    const first = store.getSnapshot().baseline;
    expect(first?.provenance).toBe("reset");
    expect(first?.entries.map((entry) => entry.id)).toEqual(["a"]);

    // A delta is not a reset boundary: no advance.
    emit({ contextUsage: null, upsert: [{ after: "a", entry: userEntry("b") }], append: [], remove: [], count: 2 }, 1);
    expect(store.getSnapshot().baseline).toBe(first);

    // A stale generation's reset is dropped before it can publish.
    emit({ contextUsage: null, reset: [userEntry("zzz")] }, 0);
    expect(store.getSnapshot().baseline).toBe(first);
    expect(store.getSnapshot().entries.map((entry) => entry.id)).toEqual(["a", "b"]);

    // Malformed frames are dropped (logged) without advancing the baseline.
    emit({ contextUsage: null, reset: "nope" } as unknown as TranscriptUpdate, 1);
    emit({ contextUsage: null, upsert: [], append: [], remove: [] } as unknown as TranscriptUpdate, 1);
    expect(store.getSnapshot().baseline).toBe(first);

    // A same-generation resubscribe: the resubscribe itself publishes
    // nothing — only the accepted reset that follows advances the epoch.
    store.resubscribe();
    expect(store.getSnapshot().baseline).toBe(first);
    emit({ contextUsage: null, reset: [userEntry("a"), userEntry("b")] }, 1);
    const second = store.getSnapshot().baseline;
    expect(second).not.toBe(first);
    expect(second!.epoch).toBeGreaterThan(first!.epoch);
    expect(second!.entries.map((entry) => entry.id)).toEqual(["a", "b"]);

    // An authoritative empty reset stays authoritative — its own baseline.
    emit({ contextUsage: null, reset: [] }, 1);
    const third = store.getSnapshot().baseline;
    expect(third!.epoch).toBeGreaterThan(second!.epoch);
    expect(third!.entries).toEqual([]);
    expect(store.getSnapshot().replay).toBe("empty");
    store.dispose();
  });

  it("the cache seed is a distinguishable baseline; the live reset supersedes it", async () => {
    const { client, emit } = fakeClient();
    const deferred = deferredCache();
    const store = new TranscriptStore(client, CHAT, { echoes: new EchoStore(), cache: deferred.cache });
    expect(store.getSnapshot().baseline).toBeNull();

    deferred.resolve({ entries: [userEntry("c")], savedAtMs: 0 });
    await Promise.resolve();
    const seed = store.getSnapshot().baseline;
    expect(seed?.provenance).toBe("seed");
    expect(seed?.entries.map((entry) => entry.id)).toEqual(["c"]);
    expect(store.getSnapshot().replay).toBe("populated");

    // The authoritative live reset is a NEW baseline, not a continuation of
    // the cache — a later reset must never be skipped as "already baselined".
    emit({ contextUsage: null, reset: [userEntry("c"), userEntry("d")] }, 1);
    const live = store.getSnapshot().baseline;
    expect(live?.provenance).toBe("reset");
    expect(live!.epoch).toBeGreaterThan(seed!.epoch);
    expect(live?.entries.map((entry) => entry.id)).toEqual(["c", "d"]);
    store.dispose();
  });

  it("a cache load resolving after the first live frame never mints a seed baseline", async () => {
    const { client, emit } = fakeClient();
    const deferred = deferredCache();
    const store = new TranscriptStore(client, CHAT, { echoes: new EchoStore(), cache: deferred.cache });

    emit({ contextUsage: null, reset: [userEntry("live")] }, 1);
    const live = store.getSnapshot().baseline;
    expect(live?.provenance).toBe("reset");

    // The late cache resolves into a loaded store: dropped entirely — no
    // seed baseline, no entry change.
    deferred.resolve({ entries: [userEntry("stale-cache")], savedAtMs: 0 });
    await Promise.resolve();
    expect(store.getSnapshot().baseline).toBe(live);
    expect(store.getSnapshot().entries.map((entry) => entry.id)).toEqual(["live"]);
    store.dispose();
  });
});

describe("sendRun mints exactly one message id", () => {
  const DRAFT: DraftConfig = {
    harness: "claude-code",
    model: "sonnet",
    reasoning: "medium",
    sandbox: "workspace-write",
    modelOptions: {},
  };

  it("the command envelope and the SendResult carry the same id", async () => {
    const mint = vi.fn(() => "msg-1");
    const calls: { method: string; params: unknown }[] = [];
    const caller = {
      call: async <T,>(method: string, params?: unknown): Promise<T> => {
        calls.push({ method, params });
        return { commandId: "cmd-1" } as T;
      },
    };

    const result = await sendRun(caller, CHAT, DRAFT, "hi", "/tmp/proj", {
      mintMessageId: mint,
    });

    // ONE mint per send — the bug this ticket fixes called it three times.
    expect(mint).toHaveBeenCalledTimes(1);
    expect(result.messageId).toBe("msg-1");
    const queued = calls.find((entry) => entry.method.toLowerCase().includes("queue"));
    expect(queued).toBeDefined();
    const command = (queued!.params as { command: { messageId: string } }).command;
    expect(command.messageId).toBe("msg-1");
  });
});
