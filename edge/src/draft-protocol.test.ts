import { describe, expect, it } from "vitest";
import {
  DRAFT_IDLE_TTL_MS,
  DRAFT_ROUTES,
  draftRoomName,
  isIdleExpired,
  MAX_CHECKPOINT_BYTES,
  MAX_ROW_BYTES,
  parseDraftPath,
  parseEpoch
} from "./draft-protocol";

describe("parseEpoch", () => {
  it("accepts positive base-10 integers", () => {
    expect(parseEpoch("1")).toBe(1);
    expect(parseEpoch("42")).toBe(42);
    expect(parseEpoch("9007199254740")).toBe(9007199254740);
  });
  it("rejects missing, zero, negative, padded, fractional and junk values", () => {
    for (const bad of [null, undefined, "", "0", "-1", "01", "+1", "1.5", "1e3", " 1", "1 ", "abc", "0x10"]) {
      expect(parseEpoch(bad)).toBeUndefined();
    }
  });
  it("rejects values beyond the safe-integer range", () => {
    expect(parseEpoch("9".repeat(17))).toBeUndefined();
    expect(parseEpoch("9007199254740993")).toBeUndefined();
  });
});

describe("parseDraftPath", () => {
  it("splits /draft/:org/:chat/:action", () => {
    expect(parseDraftPath("/draft/org1/chat-1/ws")).toEqual({
      kind: "ok",
      orgId: "org1",
      chatId: "chat-1",
      action: "ws"
    });
  });
  it("ignores non-draft paths", () => {
    expect(parseDraftPath("/chat2/x/ws")).toEqual({ kind: "notDraft" });
    expect(parseDraftPath("/drafts/o/c/ws")).toEqual({ kind: "notDraft" });
    expect(parseDraftPath("/")).toEqual({ kind: "notDraft" });
  });
  it("404s malformed draft paths", () => {
    for (const p of ["/draft", "/draft/o", "/draft/o/c", "/draft/o/c/ws/extra"]) {
      expect(parseDraftPath(p)).toEqual({ kind: "notFound" });
    }
  });
});

describe("draft constants", () => {
  it("names rooms per org, user and chat", () => {
    expect(draftRoomName("o", "u", "c")).toBe("draft1/o/u/c");
    expect(draftRoomName("o", "u2", "c")).not.toBe(draftRoomName("o", "u", "c"));
  });
  it("uses the spec limits", () => {
    expect(MAX_ROW_BYTES).toBe(64 * 1024);
    expect(MAX_CHECKPOINT_BYTES).toBe(256 * 1024);
    expect(DRAFT_IDLE_TTL_MS).toBe(30 * 24 * 3600 * 1000);
  });
  it("allow-lists methods per route", () => {
    expect(DRAFT_ROUTES.discard).toEqual(["POST"]);
    expect(DRAFT_ROUTES.epoch).toEqual(["GET"]);
    expect(DRAFT_ROUTES.rows).toEqual(["GET", "POST"]);
    expect(DRAFT_ROUTES.tail).toBeUndefined();
    expect(DRAFT_ROUTES.diff).toBeUndefined();
  });
});

describe("isIdleExpired", () => {
  it("expires exactly at the TTL", () => {
    const t0 = 1_000_000;
    expect(isIdleExpired(t0, t0 + DRAFT_IDLE_TTL_MS - 1)).toBe(false);
    expect(isIdleExpired(t0, t0 + DRAFT_IDLE_TTL_MS)).toBe(true);
  });
  it("honours an injected TTL and treats an unrecorded write as idle", () => {
    expect(isIdleExpired(100, 150, 100)).toBe(false);
    expect(isIdleExpired(100, 200, 100)).toBe(true);
    expect(isIdleExpired(0, 1)).toBe(true);
  });
});
