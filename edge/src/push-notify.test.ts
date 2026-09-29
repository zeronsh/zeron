import { describe, expect, it } from "vitest";
import { apnsPayload, chatForNotification, notificationFor, parsePrefs, SESSION_STALE_MS } from "./push-notify";
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
