import { describe, expect, it } from "vitest";
import type { Op, Row } from "./registry-core";
import { chatHost, parentOf, runnerCanSee, runnerOpRefusal } from "./registry-runner";

const CLOUD = "cloud-1";
const LAPTOP = "laptop-1";
const hlc = (n: number) => `${String(1_800_000_000_000 + n).padStart(13, "0")}-000000-x`;

const row = (kind: string, id: string, fields: Row["fields"], deleted = false): Row => ({
  kind,
  id,
  seq: 1,
  deleted,
  fields: deleted ? {} : fields,
  clocks: deleted ? {} : Object.fromEntries(Object.keys(fields).map((k) => [k, hlc(1)])),
  ...(deleted ? { delHlc: hlc(1) } : {})
});

const upsert = (kind: string, id: string, set: Op["set"], at = 5): Op => ({ kind, id, op: "upsert", set, hlc: hlc(at) });
const del = (kind: string, id: string): Op => ({ kind, id, op: "delete", hlc: hlc(9) });

describe("runner registry writes", () => {
  it("creates and edits only rows its device hosts", () => {
    expect(runnerOpRefusal(CLOUD, undefined, upsert("chats", "c1", { deviceId: CLOUD, title: "t" }))).toBeNull();
    const own = row("chats", "c1", { deviceId: CLOUD, title: "t" });
    expect(runnerOpRefusal(CLOUD, own, upsert("chats", "c1", { title: "renamed" }))).toBeNull();
    expect(runnerOpRefusal(CLOUD, own, del("chats", "c1"))).toBeNull();
    expect(runnerOpRefusal(CLOUD, row("sessions", "c1", { deviceId: CLOUD }), upsert("sessions", "c1", { status: "idle" }))).toBeNull();
  });

  it("refuses touching, claiming or deleting another device's rows", () => {
    const laptopChat = row("chats", "c2", { deviceId: LAPTOP, title: "secret" });
    expect(runnerOpRefusal(CLOUD, laptopChat, upsert("chats", "c2", { title: "x" }))).toMatch(/another device/);
    expect(runnerOpRefusal(CLOUD, laptopChat, upsert("chats", "c2", { deviceId: CLOUD }))).toMatch(/another device/);
    expect(runnerOpRefusal(CLOUD, laptopChat, del("chats", "c2"))).toMatch(/another device/);
    expect(runnerOpRefusal(CLOUD, row("spaces", "s1", { deviceId: LAPTOP }), upsert("spaces", "s1", { name: "x" }))).toMatch(/another device/);
    // Handing its own row to another device, or leaving it ownerless.
    const own = row("chats", "c1", { deviceId: CLOUD });
    expect(runnerOpRefusal(CLOUD, own, upsert("chats", "c1", { deviceId: LAPTOP }))).toMatch(/would not be hosted/);
    expect(runnerOpRefusal(CLOUD, own, upsert("chats", "c1", { deviceId: null }))).toMatch(/would not be hosted/);
    expect(runnerOpRefusal(CLOUD, undefined, upsert("chats", "c9", { title: "no host" }))).toMatch(/would not be hosted/);
    expect(runnerOpRefusal(CLOUD, undefined, upsert("chats", "c9", { deviceId: LAPTOP }))).toMatch(/would not be hosted/);
  });

  it("never revives tombstones or deletes what it cannot prove it owns", () => {
    const tomb = row("chats", "c3", {}, true);
    expect(runnerOpRefusal(CLOUD, tomb, upsert("chats", "c3", { deviceId: CLOUD }))).toMatch(/revive/);
    expect(runnerOpRefusal(CLOUD, tomb, del("chats", "c3"))).toMatch(/no row/);
    expect(runnerOpRefusal(CLOUD, undefined, del("chats", "c4"))).toMatch(/no row/);
  });

  it("owns only its own device row and no user-level kinds", () => {
    expect(runnerOpRefusal(CLOUD, undefined, upsert("devices", CLOUD, { name: "Cloud" }))).toBeNull();
    expect(runnerOpRefusal(CLOUD, row("devices", LAPTOP, { name: "Mac" }), upsert("devices", LAPTOP, { name: "x" }))).toMatch(/device row/);
    expect(runnerOpRefusal(CLOUD, undefined, del("devices", LAPTOP))).toMatch(/device row/);
    expect(runnerOpRefusal(CLOUD, undefined, upsert("preferences", "sidebar", { x: 1 }))).toMatch(/not writable/);
    expect(runnerOpRefusal(CLOUD, undefined, upsert("sidebarPins", "c1", { pinned: true }))).toMatch(/not writable/);
    expect(runnerOpRefusal(CLOUD, undefined, upsert("somethingNew", "x", { deviceId: CLOUD }))).toMatch(/not writable/);
  });
});

describe("runner registry reads", () => {
  it("sees every device, its own hosted rows, and tombstones only", () => {
    expect(runnerCanSee(CLOUD, row("devices", LAPTOP, { name: "Mac" }))).toBe(true);
    expect(runnerCanSee(CLOUD, row("chats", "c1", { deviceId: CLOUD }))).toBe(true);
    expect(runnerCanSee(CLOUD, row("chats", "c2", { deviceId: LAPTOP, title: "secret" }))).toBe(false);
    expect(runnerCanSee(CLOUD, row("sessions", "c2", { deviceId: LAPTOP }))).toBe(false);
    expect(runnerCanSee(CLOUD, row("spaces", "s1", { deviceId: LAPTOP }))).toBe(false);
    expect(runnerCanSee(CLOUD, row("preferences", "p", { a: 1 }))).toBe(false);
    expect(runnerCanSee(CLOUD, row("chats", "c3", {}, true))).toBe(true);
  });

  it("names a chat's host only for live chat rows", () => {
    expect(chatHost(row("chats", "c1", { deviceId: CLOUD }))).toBe(CLOUD);
    expect(chatHost(row("chats", "c1", {}, true))).toBeUndefined();
    expect(chatHost(row("sessions", "c1", { deviceId: CLOUD }))).toBeUndefined();
    expect(chatHost(undefined)).toBeUndefined();
  });
});

describe("delegates (child chats on other machines)", () => {
  const OTHER = "cloud-2";
  const children = new Set(["child"]);
  const child = row("chats", "child", { deviceId: OTHER, parentChatId: "main", title: "t" });

  it("sees its children's chat and session rows, never another main chat", () => {
    const scope = { spaces: new Set<string>(), children };
    expect(runnerCanSee(CLOUD, child, scope)).toBe(true);
    expect(runnerCanSee(CLOUD, row("sessions", "child", { deviceId: OTHER }), scope)).toBe(true);
    expect(runnerCanSee(CLOUD, row("chats", "other-main", { deviceId: OTHER }), scope)).toBe(false);
    expect(runnerCanSee(CLOUD, row("sessions", "other-main", { deviceId: OTHER }), scope)).toBe(false);
    expect(runnerCanSee(CLOUD, row("spaces", "s9", { deviceId: LAPTOP }), scope)).toBe(false);
    expect(runnerCanSee(CLOUD, child)).toBe(false); // no scope: hosted rows only
  });

  it("may rename, archive or delete a child, nothing else", () => {
    expect(runnerOpRefusal(CLOUD, child, upsert("chats", "child", { title: "x" }), children)).toBeNull();
    expect(runnerOpRefusal(CLOUD, child, upsert("chats", "child", { archived: true }), children)).toBeNull();
    expect(runnerOpRefusal(CLOUD, child, del("chats", "child"), children)).toBeNull();
    expect(runnerOpRefusal(CLOUD, row("sessions", "child", { deviceId: OTHER }), del("sessions", "child"), children)).toBeNull();
    expect(runnerOpRefusal(CLOUD, child, upsert("chats", "child", { deviceId: CLOUD }), children)).toMatch(/only rename/);
    expect(runnerOpRefusal(CLOUD, child, upsert("chats", "child", { parentChatId: null }), children)).toMatch(/only rename/);
    expect(runnerOpRefusal(CLOUD, child, upsert("chats", "child", { config: {} }), children)).toMatch(/only rename/);
    expect(runnerOpRefusal(CLOUD, row("sessions", "child", { deviceId: OTHER }), upsert("sessions", "child", { status: "idle" }), children)).toMatch(/another device/);
    // Not its child: refused as before.
    expect(runnerOpRefusal(CLOUD, child, upsert("chats", "child", { title: "x" }))).toMatch(/another device/);
  });

  it("reads a chat's parent", () => {
    expect(parentOf(child)).toBe("main");
    expect(parentOf(row("chats", "main", { deviceId: CLOUD }))).toBeUndefined();
    expect(parentOf(row("chats", "gone", {}, true))).toBeUndefined();
  });
});
