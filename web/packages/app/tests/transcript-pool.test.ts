import { describe, expect, it, vi } from "vitest";
import type { EngineClient, EngineWatchCache } from "@zeron/engine-client";
import type { SessionMessageEntry, TranscriptUpdate } from "@zeron/proto";
import type { StoredEngine } from "../src/lib/engine-store";
import { createEngineSession, disposeEngineSession, reconcileEngineSessions } from "../src/state/engine-session";
import { TranscriptPool, WARM_TRANSCRIPT_LIMIT } from "../src/state/transcript-pool";

function client() {
  const watches: { id: string; cancel: ReturnType<typeof vi.fn>;
    emit: (frame: TranscriptUpdate, context: { generation: number }) => void }[] = [];
  const value = { watch: (_method: string, params: { chatId: string }, handlers: {
    onItem: (frame: TranscriptUpdate, context: { generation: number }) => void;
  }) => {
    const cancel = vi.fn();
    watches.push({ id: params.chatId, cancel, emit: handlers.onItem });
    return { cancel };
  } } as unknown as EngineClient;
  return { value, watches };
}

const entry: SessionMessageEntry = {
  id: "reply", role: "assistant", status: "streaming", createdAt: 1, deviceId: "dev",
  parts: [{ kind: "text", id: "text", text: "Current output" }],
};

describe("warm transcript pool", () => {
  it("A → B → A reuses A's watch and includes streaming updates received while away", () => {
    const c = client();
    const pool = new TranscriptPool(c.value);
    const a = pool.get("A");
    c.watches[0]!.emit({ reset: [entry], contextUsage: null }, { generation: 1 });
    pool.get("B");
    const updated = { ...entry, status: "complete" as const };
    c.watches[0]!.emit({ upsert: [{ after: null, entry: updated }], append: [], remove: [], count: 1,
      contextUsage: null }, { generation: 1 });
    expect(pool.get("A")).toBe(a);
    expect(a.getSnapshot().entries).toEqual([updated]);
    expect(a.getSnapshot().baseline?.provenance).toBe("reset");
    expect(c.watches).toHaveLength(2);
    expect(c.watches[0]!.cancel).not.toHaveBeenCalled();
    pool.dispose();
  });

  it("bounds watches, refreshes recency, and reloads only evicted chats", () => {
    const c = client();
    const pool = new TranscriptPool(c.value);
    const a = pool.get("A");
    for (let i = 1; i < WARM_TRANSCRIPT_LIMIT; i += 1) pool.get(`chat-${i}`);
    pool.get("A");
    pool.get("new");
    expect(c.watches[1]!.cancel).toHaveBeenCalledOnce();
    expect(c.watches[0]!.cancel).not.toHaveBeenCalled();
    expect(pool.get("A")).toBe(a);
    pool.get("chat-1");
    expect(c.watches.filter(w => w.id === "chat-1")).toHaveLength(2);
    pool.dispose();
    pool.dispose();
    expect(c.watches.every(w => w.cancel.mock.calls.length === 1)).toBe(true);
  });

  it("retains stores through metadata refresh, isolates engines, and closes them on session replacement", () => {
    const c = client();
    const engine = { baseUrl: "https://engine.test", credential: "test" } as StoredEngine;
    const cache = {} as EngineWatchCache;
    const session = createEngineSession(engine, c.value, cache);
    const a = session.transcripts.get("A");
    const refreshed = reconcileEngineSessions(new Map([[engine.baseUrl, session]]),
      [{ ...engine, label: "Renamed" }], () => ({ client: c.value, cache }));
    expect(refreshed.displaced).toEqual([]);
    expect(refreshed.sessions.get(engine.baseUrl)!.transcripts.get("A")).toBe(a);
    const replacementClient = client();
    const replaced = reconcileEngineSessions(refreshed.sessions, [engine],
      () => ({ client: replacementClient.value, cache }));
    expect(replaced.displaced).toHaveLength(1);
    const next = replaced.sessions.get(engine.baseUrl)!;
    expect(next.transcripts.get("A")).not.toBe(a);
    for (const displaced of replaced.displaced) disposeEngineSession(displaced);
    expect(c.watches[0]!.cancel).toHaveBeenCalledOnce();
    expect(replacementClient.watches[0]!.cancel).not.toHaveBeenCalled();
    disposeEngineSession(next);
    expect(replacementClient.watches[0]!.cancel).toHaveBeenCalledOnce();
  });
});
