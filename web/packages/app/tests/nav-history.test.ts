import { describe, expect, it } from "vitest";
import {
  NAV_BOOT_ENTRY,
  NavHistory,
  NavHistoryStore,
  navEntryForPath,
  navEntryPath,
  type NavEntry,
} from "../src/state/nav-history";

/**
 * Ports of `crates/ui/src/shell.rs`'s `NavHistory` tests, name for name
 * (research 01-shell-chrome §4.1).
 */

const A: NavEntry = { kind: "chat", chatId: "a" };
const B: NavEntry = { kind: "chat", chatId: "b" };
const C: NavEntry = { kind: "chat", chatId: "c" };

describe("NavHistory", () => {
  it("nav_history_starts_with_nothing_to_walk", () => {
    const history = new NavHistory(NAV_BOOT_ENTRY);
    expect(history.canBack()).toBe(false);
    expect(history.canForward()).toBe(false);
    expect(history.len()).toBe(1);
    expect(history.current()).toEqual(NAV_BOOT_ENTRY);
    expect(history.back()).toBeNull();
    expect(history.forward()).toBeNull();
  });

  it("nav_push_then_back_and_forward", () => {
    const history = new NavHistory(NAV_BOOT_ENTRY);
    history.push(A);
    history.push(B);
    expect(history.canBack()).toBe(true);
    expect(history.canForward()).toBe(false);

    expect(history.back()).toEqual(A);
    expect(history.current()).toEqual(A);
    expect(history.canForward()).toBe(true);

    expect(history.forward()).toEqual(B);
    expect(history.current()).toEqual(B);
    expect(history.canForward()).toBe(false);
  });

  it("nav_push_dedups_the_current_route", () => {
    const history = new NavHistory(NAV_BOOT_ENTRY);
    history.push(A);
    const before = history.len();
    // By VALUE, not by reference — a fresh object naming the same route.
    history.push({ kind: "chat", chatId: "a" });
    expect(history.len()).toBe(before);
    expect(history.canForward()).toBe(false);
    expect(history.current()).toEqual(A);
  });

  it("nav_push_truncates_the_forward_branch", () => {
    const history = new NavHistory(NAV_BOOT_ENTRY);
    history.push(A);
    history.push(B);
    expect(history.back()).toEqual(A);
    history.push(C);
    expect(history.canForward()).toBe(false);
    expect(history.forward()).toBeNull();
    expect(history.current()).toEqual(C);
    // boot, A, C — B is gone.
    expect(history.len()).toBe(3);
  });

  it("nav_replace_swaps_in_place", () => {
    const history = new NavHistory(NAV_BOOT_ENTRY);
    history.push(A);
    history.push(B);
    const depth = history.len();
    history.replace(C);
    expect(history.len()).toBe(depth);
    expect(history.current()).toEqual(C);
    // Replace does not touch depth: Back still lands on whatever preceded B.
    expect(history.back()).toEqual(A);
  });

  it("nav_settings_sections_are_distinct_entries", () => {
    const history = new NavHistory(NAV_BOOT_ENTRY);
    const appearance: NavEntry = { kind: "settings", section: "appearance" };
    const devices: NavEntry = { kind: "settings", section: "devices" };
    history.push(appearance);
    history.push(devices);
    expect(history.len()).toBe(3);
    expect(history.current()).toEqual(devices);
    expect(history.back()).toEqual(appearance);
    // A settings entry never dedups against a chat entry either.
    history.push(A);
    expect(history.current()).toEqual(A);
  });
});

describe("NavHistoryStore", () => {
  it("the first selection off the untouched boot canvas replaces", () => {
    const store = new NavHistoryStore();
    store.visit(A);
    expect(store.len()).toBe(1);
    expect(store.getSnapshot().canBack).toBe(false);
    // Every later selection pushes normally.
    store.visit(B);
    expect(store.len()).toBe(2);
    expect(store.getSnapshot().canBack).toBe(true);
  });

  it("publishes a fresh snapshot to subscribers on every cursor move", () => {
    const store = new NavHistoryStore();
    let ticks = 0;
    const stop = store.subscribe(() => {
      ticks += 1;
    });
    store.visit(A);
    store.visit(B);
    const before = store.getSnapshot();
    store.back();
    expect(store.getSnapshot()).not.toBe(before);
    expect(store.getSnapshot().canForward).toBe(true);
    expect(ticks).toBe(3);
    stop();
  });

  it("nearestChat walks back from the cursor without moving it", () => {
    // `close_settings` (shell.rs:3281-3286) returns to the ACTIVE chat —
    // the web stand-in is the newest chat entry at or behind the cursor.
    const store = new NavHistoryStore();
    expect(store.nearestChat()).toEqual(NAV_BOOT_ENTRY);
    store.visit(A);
    store.visit({ kind: "settings", section: "appearance" });
    store.visit({ kind: "settings", section: "accounts" });
    // From settings: the chat behind the cursor, and the cursor did not move.
    expect(store.nearestChat()).toEqual(A);
    expect(store.current()).toEqual({ kind: "settings", section: "accounts" });
    expect(store.getSnapshot().canBack).toBe(true);
  });

  it("nearestChat ignores the forward branch the cursor walked away from", () => {
    const store = new NavHistoryStore();
    store.visit(A);
    store.visit(B);
    store.back(); // cursor on A; B sits on the forward branch
    // The walk reads the stack without moving the cursor or truncating.
    expect(store.nearestChat()).toEqual(A);
    expect(store.getSnapshot().canForward).toBe(true);
  });

  it("nearestChat falls back to the boot canvas when only settings were visited", () => {
    const store = new NavHistoryStore();
    store.visit({ kind: "settings", section: "appearance" });
    // The first settings visit REPLACED the boot entry, so there is no chat
    // behind the cursor: Back lands on the blank canvas.
    expect(store.len()).toBe(1);
    expect(store.nearestChat()).toBeNull();
  });
});

describe("nav route mapping", () => {
  it("maps paths to entries and back", () => {
    expect(navEntryForPath("/")).toEqual(NAV_BOOT_ENTRY);
    expect(navEntryForPath("/chat/abc")).toEqual({ kind: "chat", chatId: "abc" });
    expect(navEntryForPath("/chat/abc/changes")).toEqual({ kind: "chat", chatId: "abc" });
    expect(navEntryForPath("/settings/appearance")).toEqual({
      kind: "settings",
      section: "appearance",
    });
    // Outside the nav model: neither pushes nor disturbs the stack.
    expect(navEntryForPath("/pair")).toBeNull();
    expect(navEntryForPath("/files")).toBeNull();

    expect(navEntryPath(NAV_BOOT_ENTRY)).toBe("/");
    expect(navEntryPath({ kind: "chat", chatId: "abc" })).toBe("/chat/abc");
    expect(navEntryPath({ kind: "settings", section: "appearance" })).toBe("/settings/appearance");
  });
});
