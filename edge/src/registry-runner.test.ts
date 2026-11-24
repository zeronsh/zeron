import { describe, expect, it } from "vitest";
import type { Op, Row } from "./registry-core";
import { chatHost, isRunnerReach, parentOf, runnerCanSee, runnerOpRefusal, type RunnerReach } from "./registry-runner";

const CLOUD = "cloud-1";
const LAPTOP = "laptop-1";
const OTHER = "cloud-2";
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

const REACHES: RunnerReach[] = ["devices", "isolated"];
const children = new Set(["child"]);
const child = row("chats", "child", { deviceId: OTHER, parentChatId: "main", title: "t" });

describe("runner registry writes, in either reach", () => {
  for (const reach of REACHES) {
    describe(reach, () => {
      it("creates and edits rows its device hosts", () => {
        expect(runnerOpRefusal(CLOUD, undefined, upsert("chats", "c1", { deviceId: CLOUD, title: "t" }), reach)).toBeNull();
        const own = row("chats", "c1", { deviceId: CLOUD, title: "t" });
        expect(runnerOpRefusal(CLOUD, own, upsert("chats", "c1", { title: "renamed" }), reach)).toBeNull();
        expect(runnerOpRefusal(CLOUD, own, del("chats", "c1"), reach)).toBeNull();
        expect(runnerOpRefusal(CLOUD, row("sessions", "c1", { deviceId: CLOUD }), upsert("sessions", "c1", { status: "idle" }), reach)).toBeNull();
      });

      it("never claims, hands away or deletes another device's rows", () => {
        const laptopChat = row("chats", "c2", { deviceId: LAPTOP, title: "plans" });
        expect(runnerOpRefusal(CLOUD, laptopChat, upsert("chats", "c2", { deviceId: CLOUD }), reach)).not.toBeNull();
        expect(runnerOpRefusal(CLOUD, laptopChat, del("chats", "c2"), reach)).toMatch(/another device/);
        expect(runnerOpRefusal(CLOUD, row("spaces", "s1", { deviceId: LAPTOP }), upsert("spaces", "s1", { name: "x" }), reach)).toMatch(/another device/);
        expect(runnerOpRefusal(CLOUD, row("sessions", "c2", { deviceId: LAPTOP }), upsert("sessions", "c2", { status: "idle" }), reach)).toMatch(/another device/);
        const own = row("chats", "c1", { deviceId: CLOUD });
        expect(runnerOpRefusal(CLOUD, own, upsert("chats", "c1", { deviceId: LAPTOP }), reach)).toMatch(/would not be hosted/);
        expect(runnerOpRefusal(CLOUD, own, upsert("chats", "c1", { deviceId: null }), reach)).toMatch(/would not be hosted/);
        expect(runnerOpRefusal(CLOUD, undefined, upsert("chats", "c9", { title: "no host" }), reach)).toMatch(/would not be hosted/);
        expect(runnerOpRefusal(CLOUD, undefined, upsert("spaces", "s9", { deviceId: LAPTOP }), reach)).toMatch(/would not be hosted/);
        expect(runnerOpRefusal(CLOUD, undefined, upsert("sessions", "c9", { deviceId: LAPTOP }), reach)).toMatch(/would not be hosted/);
      });

      it("never revives tombstones or deletes what it cannot prove it owns", () => {
        const tomb = row("chats", "c3", {}, true);
        expect(runnerOpRefusal(CLOUD, tomb, upsert("chats", "c3", { deviceId: CLOUD }), reach)).toMatch(/revive/);
        expect(runnerOpRefusal(CLOUD, tomb, del("chats", "c3"), reach)).toMatch(/no row/);
        expect(runnerOpRefusal(CLOUD, undefined, del("chats", "c4"), reach)).toMatch(/no row/);
      });

      it("owns only its own device row and no user-level kinds", () => {
        expect(runnerOpRefusal(CLOUD, undefined, upsert("devices", CLOUD, { name: "Cloud" }), reach)).toBeNull();
        expect(runnerOpRefusal(CLOUD, row("devices", LAPTOP, { name: "Mac" }), upsert("devices", LAPTOP, { name: "x" }), reach)).toMatch(/device row/);
        expect(runnerOpRefusal(CLOUD, undefined, del("devices", LAPTOP), reach)).toMatch(/device row/);
        expect(runnerOpRefusal(CLOUD, undefined, upsert("preferences", "sidebar", { x: 1 }), reach)).toMatch(/not writable/);
        expect(runnerOpRefusal(CLOUD, undefined, upsert("sidebarPins", "c1", { pinned: true }), reach)).toMatch(/not writable/);
        expect(runnerOpRefusal(CLOUD, undefined, upsert("somethingNew", "x", { deviceId: CLOUD }), reach)).toMatch(/not writable/);
      });

      it("deletes a delegate with its session row, never re-homes it", () => {
        expect(runnerOpRefusal(CLOUD, child, upsert("chats", "child", { title: "x" }), reach, children)).toBeNull();
        expect(runnerOpRefusal(CLOUD, child, del("chats", "child"), reach, children)).toBeNull();
        expect(runnerOpRefusal(CLOUD, row("sessions", "child", { deviceId: OTHER }), del("sessions", "child"), reach, children)).toBeNull();
        expect(runnerOpRefusal(CLOUD, child, upsert("chats", "child", { deviceId: CLOUD }), reach, children)).not.toBeNull();
        expect(runnerOpRefusal(CLOUD, row("sessions", "child", { deviceId: OTHER }), upsert("sessions", "child", { status: "idle" }), reach, children)).toMatch(/another device/);
        // Not its child: no delete.
        expect(runnerOpRefusal(CLOUD, child, del("chats", "child"), reach)).toMatch(/another device/);
      });
    });
  }
});

describe("devices reach: writes chats as any of the person's devices", () => {
  it("sets any field of a chat on another device but its host, and starts chats there", () => {
    const laptopChat = row("chats", "c2", { deviceId: LAPTOP, title: "plans" });
    expect(runnerOpRefusal(CLOUD, laptopChat, upsert("chats", "c2", { title: "x" }), "devices")).toBeNull();
    expect(runnerOpRefusal(CLOUD, laptopChat, upsert("chats", "c2", { archived: true }), "devices")).toBeNull();
    expect(runnerOpRefusal(CLOUD, laptopChat, upsert("chats", "c2", { branch: "dev", config: { harness: "codex" } }), "devices")).toBeNull();
    expect(runnerOpRefusal(CLOUD, undefined, upsert("chats", "c9", { deviceId: LAPTOP, parentChatId: "c1" }), "devices")).toBeNull();
    expect(runnerOpRefusal(CLOUD, laptopChat, upsert("chats", "c2", { deviceId: CLOUD }), "devices")).toMatch(/stays hosted there/);
    expect(runnerOpRefusal(CLOUD, laptopChat, upsert("chats", "c2", { title: "x", deviceId: LAPTOP }), "devices")).toMatch(/stays hosted there/);
  });
});

describe("isolated reach: acts as its device alone", () => {
  it("touches no chat on another device, and starts none there", () => {
    const laptopChat = row("chats", "c2", { deviceId: LAPTOP, title: "secret" });
    expect(runnerOpRefusal(CLOUD, laptopChat, upsert("chats", "c2", { title: "x" }), "isolated")).toMatch(/another device/);
    expect(runnerOpRefusal(CLOUD, laptopChat, upsert("chats", "c2", { archived: true }), "isolated")).toMatch(/another device/);
    expect(runnerOpRefusal(CLOUD, undefined, upsert("chats", "c9", { deviceId: LAPTOP }), "isolated")).toMatch(/would not be hosted/);
  });

  it("may only rename, archive or delete a delegate", () => {
    expect(runnerOpRefusal(CLOUD, child, upsert("chats", "child", { archived: true }), "isolated", children)).toBeNull();
    expect(runnerOpRefusal(CLOUD, child, upsert("chats", "child", { parentChatId: null }), "isolated", children)).toMatch(/only rename/);
    expect(runnerOpRefusal(CLOUD, child, upsert("chats", "child", { config: {} }), "isolated", children)).toMatch(/only rename/);
    expect(runnerOpRefusal(CLOUD, child, upsert("chats", "child", { title: "x" }), "isolated")).toMatch(/another device/);
  });
});

describe("runner registry reads", () => {
  it("devices: every device, chat, project and session, none of the user's own kinds", () => {
    expect(runnerCanSee(CLOUD, row("devices", LAPTOP, { name: "Mac" }), "devices")).toBe(true);
    expect(runnerCanSee(CLOUD, row("chats", "c1", { deviceId: CLOUD }), "devices")).toBe(true);
    expect(runnerCanSee(CLOUD, row("chats", "c2", { deviceId: LAPTOP, title: "plans" }), "devices")).toBe(true);
    expect(runnerCanSee(CLOUD, row("sessions", "c2", { deviceId: LAPTOP }), "devices")).toBe(true);
    expect(runnerCanSee(CLOUD, row("spaces", "s1", { deviceId: LAPTOP }), "devices")).toBe(true);
    expect(runnerCanSee(CLOUD, row("chats", "c3", {}, true), "devices")).toBe(true);
    expect(runnerCanSee(CLOUD, row("preferences", "p", { a: 1 }), "devices")).toBe(false);
    expect(runnerCanSee(CLOUD, row("sidebarPins", "c1", { pinned: true }), "devices")).toBe(false);
  });

  it("both reaches read the account's plugin switches, and never write them", () => {
    const sw = row("pluginSwitches", "orchestrators", { on: true });
    expect(runnerCanSee(CLOUD, sw, "devices")).toBe(true);
    expect(runnerCanSee(CLOUD, sw, "isolated")).toBe(true);
    for (const reach of ["devices", "isolated"] as const) {
      expect(runnerOpRefusal(CLOUD, sw, upsert("pluginSwitches", "orchestrators", { on: false }), reach)).toMatch(/not writable/);
      expect(runnerOpRefusal(CLOUD, undefined, upsert("pluginSwitches", "orchestrators", { on: true }), reach)).toMatch(/not writable/);
    }
  });

  it("isolated: every device, its own rows, its projects and delegates, and tombstones", () => {
    const scope = { spaces: new Set(["s1"]), children };
    expect(runnerCanSee(CLOUD, row("devices", LAPTOP, { name: "Mac" }), "isolated")).toBe(true);
    expect(runnerCanSee(CLOUD, row("chats", "c1", { deviceId: CLOUD }), "isolated")).toBe(true);
    expect(runnerCanSee(CLOUD, row("chats", "c2", { deviceId: LAPTOP, title: "secret" }), "isolated", scope)).toBe(false);
    expect(runnerCanSee(CLOUD, row("sessions", "c2", { deviceId: LAPTOP }), "isolated", scope)).toBe(false);
    expect(runnerCanSee(CLOUD, row("spaces", "s1", { deviceId: LAPTOP }), "isolated", scope)).toBe(true);
    expect(runnerCanSee(CLOUD, row("spaces", "s2", { deviceId: LAPTOP }), "isolated", scope)).toBe(false);
    expect(runnerCanSee(CLOUD, child, "isolated", scope)).toBe(true);
    expect(runnerCanSee(CLOUD, row("sessions", "child", { deviceId: OTHER }), "isolated", scope)).toBe(true);
    expect(runnerCanSee(CLOUD, row("chats", "other-main", { deviceId: OTHER }), "isolated", scope)).toBe(false);
    expect(runnerCanSee(CLOUD, child, "isolated")).toBe(false); // no scope: hosted rows only
    expect(runnerCanSee(CLOUD, row("chats", "c3", {}, true), "isolated")).toBe(true);
    expect(runnerCanSee(CLOUD, row("preferences", "p", { a: 1 }), "isolated")).toBe(false);
  });

  it("names a chat's host and parent only for live chat rows", () => {
    expect(chatHost(row("chats", "c1", { deviceId: CLOUD }))).toBe(CLOUD);
    expect(chatHost(row("chats", "c1", {}, true))).toBeUndefined();
    expect(chatHost(row("sessions", "c1", { deviceId: CLOUD }))).toBeUndefined();
    expect(chatHost(undefined)).toBeUndefined();
    expect(parentOf(child)).toBe("main");
    expect(parentOf(row("chats", "main", { deviceId: CLOUD }))).toBeUndefined();
    expect(parentOf(row("chats", "gone", {}, true))).toBeUndefined();
  });

  it("knows the two reaches", () => {
    expect(isRunnerReach("devices")).toBe(true);
    expect(isRunnerReach("isolated")).toBe(true);
    expect(isRunnerReach("everything")).toBe(false);
    expect(isRunnerReach(undefined)).toBe(false);
  });
});
