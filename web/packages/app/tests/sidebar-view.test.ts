import { describe, expect, it } from "vitest";
import type { Chat } from "@zeron/proto";
import {
  chatRowHeight,
  compareSidebarChats,
  promoteLocalDeviceGroup,
  resortOffsets,
  sidebarGroups,
  sidebarKeyOrderChanged,
  sidebarRowHeight,
  sidebarVisibleOrder,
  type ChatRow,
  type SidebarBucket,
  type SidebarKeyed,
} from "../src/lib/view";
import type { SidebarSection } from "../src/state/ui-settings";
import {
  SIDEBAR_ACTIVE_HARNESS_ICON_SIZE,
  SIDEBAR_ACTIVE_HARNESS_TITLE_GAP,
  SIDEBAR_RESORT_CURVE,
  SIDEBAR_RESORT_MS,
} from "../src/components/chat-list";
import {
  SIDEBAR_DISCLOSURE_MS,
  disclosureAnimating,
  disclosureCurrent,
  type DisclosureMotion,
} from "../src/components/sidebar-disclosure";

const NOW = Date.parse("2026-09-16T12:00:00Z");

/*
 * The sidebar's pure logic, ported against the desktop tests it mirrors
 * (spaces.rs and shell.rs §"sidebar resort FLIP diff"). Names follow the
 * Rust tests one-for-one so the two suites read side by side.
 */

describe("compareSidebarChats", () => {
  it("equal_sidebar_timestamps_sort_by_stable_chat_id", () => {
    const alpha = chat("alpha");
    const beta = chat("beta");
    expect(compareSidebarChats("created", alpha, beta)).toBeLessThan(0);
    expect(compareSidebarChats("lastUpdated", alpha, beta)).toBeLessThan(0);
  });

  it("lastUpdated keys on lastMessageAt with createdAt fallback", () => {
    const quiet = chat("quiet", { createdAt: "2026-09-16T10:00:00Z" });
    const active = chat("active", {
      createdAt: "2026-09-16T09:00:00Z",
      lastMessageAt: "2026-09-16T11:00:00Z",
    });
    expect(compareSidebarChats("lastUpdated", active, quiet)).toBeLessThan(0);
    // "created" ignores the message: the earlier-created chat sorts last.
    expect(compareSidebarChats("created", active, quiet)).toBeGreaterThan(0);
  });
});

describe("promoteLocalDeviceGroup", () => {
  it("current_device_is_promoted_without_resorting_remote_groups", () => {
    const groups = [
      bucket("recent-remote"),
      bucket("local"),
      bucket("older-remote"),
    ];
    expect(
      promoteLocalDeviceGroup(groups, "local").map((entry) => entry.group!.key),
    ).toEqual(["local", "recent-remote", "older-remote"]);
  });

  it("missing_current_device_leaves_group_order_untouched", () => {
    const groups = [bucket("first"), bucket("second")];
    expect(promoteLocalDeviceGroup(groups, "not-present")).toEqual(groups);
  });
});

describe("chatRowHeight", () => {
  it("sidebar_chat_height_tracks_visible_metadata", () => {
    expect(chatRowHeight(false, false)).toBe(45);
    expect(chatRowHeight(true, false)).toBe(61);
    expect(chatRowHeight(false, true)).toBe(63);
    expect(chatRowHeight(true, true)).toBe(63);
  });
});

describe("sidebarRowHeight (upstream 78e9e6ae)", () => {
  it("compact_rows_are_29px_regardless_of_metadata", () => {
    expect(sidebarRowHeight(true, true, false, false)).toBe(29);
    expect(sidebarRowHeight(true, false, true, true)).toBe(29);
  });

  it("detailed_rows_lose_16px_when_the_location_label_hides", () => {
    expect(sidebarRowHeight(false, true, false, false)).toBe(45);
    expect(sidebarRowHeight(false, false, false, false)).toBe(29);
    expect(sidebarRowHeight(false, true, true, false)).toBe(61);
    expect(sidebarRowHeight(false, false, true, false)).toBe(45);
  });
});

describe("harness geometry", () => {
  it("sidebar_harness_geometry_reflects_row_hierarchy", () => {
    expect(SIDEBAR_ACTIVE_HARNESS_TITLE_GAP).toBe(8);
  });
});

describe("sidebarKeyOrderChanged", () => {
  it("sidebar_height_change_is_not_a_reorder", () => {
    const open = keys([
      ["first-group", 105],
      ["second-group", 240],
    ]);
    const collapsed = keys([
      ["first-group", 40],
      ["second-group", 240],
    ]);
    expect(sidebarKeyOrderChanged(open, collapsed)).toBe(false);

    const reordered = keys([
      ["second-group", 240],
      ["first-group", 40],
    ]);
    expect(sidebarKeyOrderChanged(collapsed, reordered)).toBe(true);
  });
});

describe("resortOffsets", () => {
  it("resort_offsets_empty_when_order_unchanged", () => {
    const order = keys([
      ["a", 29],
      ["b", 29],
      ["c", 45],
    ]);
    expect(resortOffsets(order, order, 2).size).toBe(0);
  });

  it("resort_offsets_activity_moves_row_to_top", () => {
    const old = keys([
      ["a", 29],
      ["b", 29],
      ["c", 29],
    ]);
    const next = keys([
      ["c", 29],
      ["a", 29],
      ["b", 29],
    ]);
    const offsets = resortOffsets(old, next, 2);
    expect(offsets.get("c")).toBe(62);
    expect(offsets.get("a")).toBe(-31);
    expect(offsets.get("b")).toBe(-31);
  });

  it("resort_offsets_respect_heights_and_gap", () => {
    const old = keys([
      ["tall", 45],
      ["short", 29],
    ]);
    const next = keys([
      ["short", 29],
      ["tall", 45],
    ]);
    const offsets = resortOffsets(old, next, 2);
    expect(offsets.get("short")).toBe(47);
    expect(offsets.get("tall")).toBe(-31);
  });

  it("resort_offsets_ignore_added_and_removed_keys", () => {
    const old = keys([
      ["a", 29],
      ["gone", 29],
      ["b", 29],
    ]);
    const next = keys([
      ["new", 29],
      ["a", 29],
      ["b", 29],
    ]);
    const offsets = resortOffsets(old, next, 2);
    expect(offsets.has("new")).toBe(false);
    expect(offsets.has("gone")).toBe(false);
    expect(offsets.get("a")).toBe(-31);
    expect(offsets.get("b")).toBeUndefined();
  });
});

describe("resort glide spec", () => {
  it("resort_glide_spec_matches_original", () => {
    expect(SIDEBAR_RESORT_MS).toBe(260);
    expect([...SIDEBAR_RESORT_CURVE]).toEqual([0.22, 1, 0.36, 1]);
  });
});

describe("sidebarGroups / sidebarVisibleOrder", () => {
  it("groups by device in first-seen order and promotes the local group", () => {
    const rows = chatRows([
      chat("r1", { deviceId: "remote-1" }),
      chat("l1", { deviceId: "local" }),
      chat("r2", { deviceId: "remote-2" }),
      chat("r1b", { deviceId: "remote-1" }),
    ]);
    const grouped = sidebarGroups(rows, "byDevice", "local");
    expect(grouped.map((entry) => entry.group!.key)).toEqual([
      "local",
      "remote-1",
      "remote-2",
    ]);
    expect(grouped[0]!.rows.map((row) => row.chat.id)).toEqual(["l1"]);
    expect(grouped[1]!.rows.map((row) => row.chat.id)).toEqual(["r1", "r1b"]);
    // The flat order — what the jump shortcuts would consume — is the same
    // draw order, headers not counted.
    expect(sidebarVisibleOrder(rows, "byDevice", "local")).toEqual([
      "l1",
      "r1",
      "r1b",
      "r2",
    ]);
  });

  it("inOneList emits one headerless bucket in sort order", () => {
    const rows = chatRows([
      chat("b", { lastMessageAt: "2026-09-16T11:30:00Z" }),
      chat("a", { lastMessageAt: "2026-09-16T11:00:00Z" }),
    ]);
    const grouped = sidebarGroups(rows, "inOneList", "local");
    expect(grouped).toHaveLength(1);
    expect(grouped[0]!.group).toBe(null);
    expect(grouped[0]!.rows.map((row) => row.chat.id)).toEqual(["b", "a"]);
    expect(sidebarVisibleOrder(rows, "inOneList", null)).toEqual(["b", "a"]);
  });

  it("byProject groups by space in first-seen order (upstream 78e9e6ae)", () => {
    const rows = chatRows([
      chat("p1b", { spaceId: "proj-1", deviceId: "local" }),
      chat("p2a", { spaceId: "proj-2", deviceId: "remote-1" }),
      chat("p1a", { spaceId: "proj-1", deviceId: "local" }),
      chat("home1", { deviceId: "remote-2" }),
    ]);
    const grouped = sidebarGroups(rows, "byProject", "local");
    expect(grouped.map((entry) => entry.group!.key)).toEqual(["proj-1", "proj-2", "home:remote-2"]);
    expect(grouped.every((entry) => entry.group!.kind === "project")).toBe(true);
    expect(grouped[0]!.rows.map((row) => row.chat.id)).toEqual(["p1b", "p1a"]);
    // The project-less session reads as its home group; no local promotion
    // under byProject (upstream 78e9e6ae promotes device groups only).
    expect(sidebarVisibleOrder(rows, "byProject", "local")).toEqual([
      "p1b",
      "p1a",
      "p2a",
      "home1",
    ]);
  });

  it("the displayed order leads with pins in saved order (sidebar_visible_order)", () => {
    const rows = chatRows([
      chat("r1", { deviceId: "remote-1" }),
      chat("l1", { deviceId: "local" }),
      chat("r2", { deviceId: "remote-2" }),
    ]);
    // Pins jump the device grouping; unpinned rows keep the grouped order.
    expect(sidebarVisibleOrder(rows, "byDevice", "local", ["r2", "l1"])).toEqual(["r2", "l1", "r1"]);
    // A pin on a chat that is gone never disturbs the rest.
    expect(sidebarVisibleOrder(rows, "inOneList", null, ["gone", "r1"])).toEqual(["r1", "l1", "r2"]);
  });

  it("a collapsed pinned section holds no slot (sidebar_visible_order)", () => {
    const rows = chatRows([
      chat("r1", { deviceId: "remote-1" }),
      chat("l1", { deviceId: "local" }),
      chat("r2", { deviceId: "remote-2" }),
    ]);
    // Collapsed, the hidden pins drop out and the regular rows take slots
    // from 0 — `spaces.rs`'s `!pinned_open` retain.
    expect(sidebarVisibleOrder(rows, "byDevice", "local", ["r2", "l1"], false)).toEqual(["r1"]);
    // An empty pin list is indifferent to the disclosure.
    expect(sidebarVisibleOrder(rows, "byDevice", "local", [], false)).toEqual(["l1", "r1", "r2"]);
  });

  // ── Custom sections (upstream 86249cf0's `sidebar_visible_order`) ──────

  it("open sections slot their members between the pins and the unclaimed rows", () => {
    const rows = chatRows([
      chat("s1", { deviceId: "local" }),
      chat("r1", { deviceId: "local" }),
      chat("r2", { deviceId: "local" }),
    ]);
    const sections: readonly SidebarSection[] = [
      { id: "a", name: "A", sessionIds: ["s1", "gone"], collapsed: false },
      { id: "b", name: "B", sessionIds: [], collapsed: false },
    ];
    // The caller masks claimed pins first (`active_sidebar_pins`); the
    // open sections' EXISTING members follow the pins, then the unclaimed
    // rows keep their grouped order; a vanished member holds no slot.
    expect(sidebarVisibleOrder(rows, "inOneList", null, ["r2"], true, sections)).toEqual([
      "r2",
      "s1",
      "r1",
    ]);
  });

  it("a collapsed section's members hold no slot", () => {
    const rows = chatRows([
      chat("s1", { deviceId: "local" }),
      chat("r1", { deviceId: "local" }),
    ]);
    const sections: readonly SidebarSection[] = [
      { id: "a", name: "A", sessionIds: ["s1"], collapsed: true },
    ];
    // Collapsed, s1 is neither in the section order nor in the regular
    // groups — it is simply not on the screen.
    expect(sidebarVisibleOrder(rows, "inOneList", null, [], true, sections)).toEqual(["r1"]);
    // No sections at all: the legacy call shape (equal sort keys keep the
    // projection's stable input order).
    expect(sidebarVisibleOrder(rows, "inOneList", null, [], true)).toEqual(["s1", "r1"]);
  });

  it("section members never fall through to the regular order", () => {
    const rows = chatRows([
      chat("s1", { deviceId: "remote-1" }),
      chat("l1", { deviceId: "local" }),
    ]);
    const sections: readonly SidebarSection[] = [
      { id: "a", name: "A", sessionIds: ["s1"], collapsed: false },
    ];
    expect(sidebarVisibleOrder(rows, "byDevice", "local", [], true, sections)).toEqual(["s1", "l1"]);
    expect(sidebarVisibleOrder(rows, "byDevice", "local", [], false, sections)).toEqual(["s1", "l1"]);
  });
});

describe("disclosure motion", () => {
  it("sidebar_disclosure_motion_lands_exactly_on_its_target", () => {
    const motion: DisclosureMotion = { epoch: 1, from: 240, to: 0, startedAt: 0 };
    const after = 2 * SIDEBAR_DISCLOSURE_MS;
    expect(disclosureCurrent(motion, after)).toBe(0);
    expect(disclosureAnimating(motion, after)).toBe(false);
    // A mid-flight epoch bump captures the in-flight height as its `from`
    // and still lands exactly on its own target.
    const mid = disclosureCurrent(motion, SIDEBAR_DISCLOSURE_MS / 2);
    const bumped: DisclosureMotion = {
      epoch: 2,
      from: mid,
      to: 240,
      startedAt: SIDEBAR_DISCLOSURE_MS / 2,
    };
    expect(disclosureCurrent(bumped, SIDEBAR_DISCLOSURE_MS / 2 + 2 * SIDEBAR_DISCLOSURE_MS)).toBe(240);
  });
});

// ---- helpers ----

function chat(id: string, fields: Partial<Chat> = {}): Chat {
  return {
    id,
    deviceId: "device-1",
    title: null,
    archived: false,
    cwd: null,
    branch: null,
    checkoutId: null,
    config: null,
    lastMessagePreview: null,
    lastMessageAt: null,
    createdAt: "2026-09-16T10:00:00Z",
    ...fields,
  };
}

function bucket(deviceId: string): SidebarBucket<ChatRow> {
  return { group: { key: deviceId, label: deviceId, kind: "device" }, rows: [] };
}

function keys(list: readonly (readonly [string, number])[]): SidebarKeyed[] {
  return list.map(([key, height]) => ({ key, height }));
}

function chatRows(chats: readonly Chat[]): ChatRow[] {
  // A minimal ChatRow shell: the grouping/order functions only read chat,
  // deviceId and deviceName.
  return chats.map((entry) => ({
    chat: entry,
    status: "idle" as const,
    project: "~",
    projectPath: null,
    folder: "~",
    harness: null,
    branch: null,
    timeAgo: "now",
    deviceId: entry.deviceId,
    deviceName: null,
    deviceOffline: false,
    changeRequest: null,
  }));
}
