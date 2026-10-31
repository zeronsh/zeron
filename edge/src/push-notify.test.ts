import { describe, expect, it } from "vitest";
import { apnsPayload, chatForNotification, decideNotification, notificationFor, parsePrefs, SESSION_STALE_MS } from "./push-notify";
import type { Row } from "./registry-core";

const NOW = 1_800_000_000_000;

const session = (status: string, turn: string | null, updatedAt = NOW): Row => ({
  kind: "sessions",
  id: "c1",
  seq: 1,
  deleted: false,
  fields: { chatId: "c1", status, updatedAt, ...(turn === null ? {} : { lastCompletedTurn: turn }) },
  clocks: {}
});

// The desktop's cases (crates/ui/src/sound.rs tests), row for row.
describe("notificationFor (desktop parity)", () => {
  it("a completed turn notifies once", () => {
    const working = session("working", null);
    const done = session("idle", "t1");
    expect(notificationFor(working, done, NOW)).toBe("done");
    expect(notificationFor(done, done, NOW)).toBeNull();
  });

  it("interrupted and expired activity never notifies", () => {
    expect(notificationFor(session("working", "old"), session("idle", "old"), NOW)).toBeNull();
    expect(notificationFor(session("working", null), session("idle", null), NOW)).toBeNull();
  });

  it("a run error notifies once and never masquerades as completion", () => {
    const errored = session("errored", "failed");
    expect(notificationFor(session("working", "old"), errored, NOW)).toBe("failed");
    expect(notificationFor(errored, errored, NOW)).toBeNull();
  });

  it("a question notifies once", () => {
    const asking = session("awaitingInput", "t1");
    expect(notificationFor(session("working", "t1"), asking, NOW)).toBe("input");
    expect(notificationFor(asking, asking, NOW)).toBeNull();
  });

  it("queued completions survive coalesced working states", () => {
    const first = session("working", null);
    const second = session("working", "first");
    expect(notificationFor(first, second, NOW)).toBe("done");
    expect(notificationFor(second, second, NOW)).toBeNull();
    const last = session("idle", "second");
    expect(notificationFor(second, last, NOW)).toBe("done");
  });

  it("a stale completion (old heartbeat replayed) is silent", () => {
    const stale = session("idle", "old", NOW - SESSION_STALE_MS - 1);
    expect(notificationFor(session("working", null), stale, NOW)).toBeNull();
  });

  it("first sight of a row only sets the baseline", () => {
    expect(notificationFor(undefined, session("errored", "t"), NOW)).toBeNull();
    expect(notificationFor({ ...session("idle", null), deleted: true }, session("idle", "t"), NOW)).toBeNull();
  });

  it("a question on a dead (stale) session is not news", () => {
    expect(notificationFor(session("working", null), session("awaitingInput", null, NOW - 60_000), NOW)).toBeNull();
  });
});

// The engine's held finish (crates/engine/src/desktop_notify.rs tests).
describe("decideNotification (finish held for running subagents)", () => {
  const row = (status: string, turn: string | null, count: number, updatedAt = NOW): Row => {
    const base = session(status, turn, updatedAt);
    return { ...base, fields: { ...base.fields, runningSubagents: count } };
  };

  /** Feeds rows in order like RegistryRoom does, carrying the hold; the
   * categories each change rang. */
  const play = (rows: Row[]): Array<string | null> => {
    let held: string | undefined;
    return rows.slice(1).map((after, i) => {
      const decision = decideNotification(rows[i], after, NOW, held);
      held = decision.held;
      return decision.category;
    });
  };

  it("a finish with running subagents rings once they are done", () => {
    expect(
      play([row("working", null, 2), row("idle", "t1", 2), row("idle", "t1", 1), row("idle", "t1", 0), row("idle", "t1", 0)])
    ).toEqual([null, null, "done", null]);
  });

  it("a wake turn rings once for the held finish", () => {
    // The engine settles the wake turn before publishing the drain.
    expect(
      play([row("working", null, 1), row("idle", "t1", 1), row("working", "t1", 1), row("idle", "t2", 1), row("idle", "t2", 0)])
    ).toEqual([null, null, null, "done"]);
  });

  it("a completion reported while working rings once, not again on the drain", () => {
    expect(
      play([row("working", "old", 0), row("working", "t1", 1), row("idle", "t1", 1), row("idle", "t1", 0)])
    ).toEqual(["done", null, null]);
  });

  it("subagents starting after a finish rang never ring it again", () => {
    expect(play([row("working", null, 0), row("idle", "t1", 0), row("idle", "t1", 1), row("idle", "t1", 0)])).toEqual([
      "done",
      null,
      null
    ]);
  });

  it("a question, failure or new run supersedes the held finish", () => {
    const held = [row("working", null, 1), row("idle", "t1", 1)];
    expect(play([...held, row("awaitingInput", "t1", 1), row("idle", "t1", 0)])).toEqual([null, "input", null]);
    expect(play([...held, row("errored", "t1", 1), row("idle", "t1", 0)])).toEqual([null, "failed", null]);
    // A run started and was interrupted: no new completion, the hold is gone.
    expect(play([...held, row("working", "t1", 1), row("idle", "t1", 1), row("idle", "t1", 0)])).toEqual([
      null,
      null,
      null,
      null
    ]);
  });

  it("a stale row drops the hold for good", () => {
    const stale = row("idle", "t1", 1, NOW - SESSION_STALE_MS - 1);
    expect(play([row("working", null, 1), row("idle", "t1", 1), stale, row("idle", "t1", 1), row("idle", "t1", 0)])).toEqual([
      null,
      null,
      null,
      null
    ]);
  });
});

describe("chatForNotification", () => {
  const chat = (fields: Row["fields"]): Row => ({ kind: "chats", id: "c1", seq: 1, deleted: false, fields, clocks: {} });
  it("titles by the chat, falling back like desktop", () => {
    expect(chatForNotification(chat({ title: "Fix login" }))).toEqual({ title: "Fix login" });
    expect(chatForNotification(chat({}))).toEqual({ title: "New session" });
    expect(chatForNotification(undefined)).toEqual({ title: "New session" });
  });
  it("skips side chats and archived chats", () => {
    expect(chatForNotification(chat({ parentChatId: "p" }))).toBeNull();
    expect(chatForNotification(chat({ archived: true }))).toBeNull();
  });
  it("a snoozed chat skips done but still asks and fails", () => {
    const snoozed = chat({ title: "Later", snoozedUntil: NOW + 60_000 });
    expect(chatForNotification(snoozed, "done", NOW)).toBeNull();
    expect(chatForNotification(snoozed, "input", NOW)).toEqual({ title: "Later" });
    expect(chatForNotification(snoozed, "failed", NOW)).toEqual({ title: "Later" });
    // Past wake time (and a cleared snooze) notify as usual.
    expect(chatForNotification(snoozed, "done", NOW + 60_000)).toEqual({ title: "Later" });
    expect(chatForNotification(chat({ title: "Later", snoozedUntil: null }), "done", NOW)).toEqual({ title: "Later" });
  });
});

describe("prefs and payload", () => {
  it("defaults every category on", () => {
    expect(parsePrefs(undefined)).toEqual({ done: true, input: true, failed: true });
    expect(parsePrefs({ done: false })).toEqual({ done: false, input: true, failed: true });
  });
  it("carries the desktop's words and the chat to open", () => {
    const p = apnsPayload("c1", "Fix login", "input");
    expect(p.aps.alert).toEqual({ title: "Fix login", body: "Waiting on your input" });
    expect(p.chatId).toBe("c1");
    expect(p.aps["thread-id"]).toBe("c1");
  });
});
