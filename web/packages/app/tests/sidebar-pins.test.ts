import { describe, expect, it } from "vitest";
import {
  commitSessionDrop,
  commitVisiblePinReorder,
  pinOrderedRows,
  pinnedDragScrollDelta,
  pinnedDragScrollStep,
  pinnedDragSnapshotIsValid,
  pinnedSectionBodyHeight,
  pinnedHeaderKeyedHeight,
  pinnedSessionClampedIndex,
  pinnedSessionDropIndex,
  projectPinnedFirst,
  projectSidebarPinChange,
  reorderVisiblePins,
  retainKnownPins,
  sidebarGapOffset,
  sidebarPinProfileKey,
  sidebarSessionDropChange,
  sidebarSessionDropPins,
  SIDEBAR_PINNED_DIVIDER_FRAME_HEIGHT,
  SIDEBAR_SESSION_SLOT,
} from "../src/lib/sidebar-pins";
import type { ChatRow } from "../src/lib/view";

/*
 * The pinned-section's pure logic, ported against the desktop tests it
 * mirrors (`spaces.rs`'s `pinned_session_tests`, upstream zeron fd42e2ab).
 * Names follow the Rust tests one-for-one so the two suites read side by
 * side.
 */

describe("projectPinnedFirst", () => {
  it("pins_lead_without_changing_unpinned_recency", () => {
    const recency = ["newest", "p2", "middle", "p1", "oldest"];
    expect(projectPinnedFirst(recency, ["p1", "p2"])).toEqual(["p1", "p2", "newest", "middle", "oldest"]);
  });

  it("missing_duplicate_and_archived_pins_do_not_disturb_regular_rows", () => {
    const recency = ["b", "a", "c"];
    expect(projectPinnedFirst(recency, ["archived", "a", "a"])).toEqual(["a", "b", "c"]);
  });
});

describe("reorderVisiblePins", () => {
  it("filtered_pin_reorder_preserves_every_other_pins_relative_order", () => {
    const saved = ["a1", "b1", "a2", "archived", "b2"];
    const visible = ["a1", "a2"];
    // Only the dragged pin moves; the hidden/archived pins and every other
    // pin keep their relative order (68306a17's per-item moves).
    expect(reorderVisiblePins(saved, visible, 0, 1)).toEqual([
      "b1",
      "a2",
      "a1",
      "archived",
      "b2",
    ]);
  });

  it("pin_reorder_rejects_invalid_or_noop_moves", () => {
    const saved = ["a", "b"];
    expect(reorderVisiblePins(saved, saved, 0, 0)).toEqual(saved);
    expect(reorderVisiblePins(saved, saved, 8, 0)).toEqual(saved);
  });
});

describe("projectSidebarPinChange", () => {
  it("pending_move_does_not_revive_unpinned_item", () => {
    expect(
      projectSidebarPinChange(["remote"], {
        action: "move",
        sessionId: "gone",
        after: null,
        before: null,
      }),
    ).toEqual(["remote"]);
  });

  it("intents_rebase_onto_the_latest_projection", () => {
    // Pin between the two; a surviving right anchor wins over the left.
    expect(
      projectSidebarPinChange(["a", "b"], {
        action: "pin",
        sessionId: "mid",
        after: "a",
        before: "b",
      }),
    ).toEqual(["a", "mid", "b"]);
    // Both anchors gone: append.
    expect(
      projectSidebarPinChange(["a", "mid", "b"], {
        action: "pin",
        sessionId: "tail",
        after: "gone",
        before: "also-gone",
      }),
    ).toEqual(["a", "mid", "b", "tail"]);
    // Unpin removes.
    expect(
      projectSidebarPinChange(["a", "mid", "b", "tail"], {
        action: "unpin",
        sessionId: "mid",
      }),
    ).toEqual(["a", "b", "tail"]);
  });
});

describe("sidebarSessionDropChange", () => {
  it("a drop becomes one per-item intent anchored to its neighbors", () => {
    const saved = ["hidden", "a", "b", "hidden-tail"];
    // An existing pin moving: Move with the drop's neighbors.
    expect(sidebarSessionDropChange(saved, ["hidden", "b", "a", "hidden-tail"], "a")).toEqual({
      action: "move",
      sessionId: "a",
      after: "b",
      before: "hidden-tail",
    });
    // A new pin: Pin with its neighbors.
    expect(sidebarSessionDropChange(saved, ["hidden", "normal", "a", "b", "hidden-tail"], "normal")).toEqual({
      action: "pin",
      sessionId: "normal",
      after: "hidden",
      before: "a",
    });
    // An absent id: Unpin.
    expect(sidebarSessionDropChange(saved, ["hidden", "b", "hidden-tail"], "a")).toEqual({
      action: "unpin",
      sessionId: "a",
    });
  });
});

describe("retainKnownPins", () => {
  it("pin_cleanup_retains_archived_and_prunes_deleted", () => {
    const known = new Set(["active", "archived"]);
    expect(retainKnownPins(["active", "archived", "deleted", "active"], known)).toEqual(["active", "archived"]);
    expect(retainKnownPins(["active", "archived"], known)).toBe(null);
  });
});

describe("pinnedSessionDropIndex", () => {
  it("pinned_drop_index_quantizes_clamps_and_rejects_outside", () => {
    expect(pinnedSessionDropIndex(-1, 3)).toBe(null);
    expect(pinnedSessionDropIndex(0, 3)).toBe(0);
    expect(pinnedSessionDropIndex(SIDEBAR_SESSION_SLOT, 3)).toBe(1);
    expect(pinnedSessionDropIndex(500, 3)).toBe(null);
    expect(pinnedSessionDropIndex(0, 0)).toBe(null);
  });
});

describe("pinnedSessionClampedIndex", () => {
  it("sidebar_wide_pin_drag_clamps_to_the_nearest_pinned_slot", () => {
    expect(pinnedSessionClampedIndex(-50, 3)).toBe(0);
    expect(pinnedSessionClampedIndex(SIDEBAR_SESSION_SLOT, 3)).toBe(1);
    expect(pinnedSessionClampedIndex(500, 3)).toBe(2);
    expect(pinnedSessionClampedIndex(0, 0)).toBe(null);
  });
});

describe("pinned drag autoscroll", () => {
  it("pinned_edge_scroll_is_proportional_and_lifecycle_bound", () => {
    const top = 100;
    const bottom = 300;
    expect(pinnedDragScrollDelta(200, top, bottom)).toBe(0);
    expect(pinnedDragScrollDelta(124, top, bottom)).toBe(-6);
    expect(pinnedDragScrollDelta(276, top, bottom)).toBe(6);
    expect(pinnedDragScrollStep(true, 4, 4, 20, 100, 6)).toBe(26);
    expect(pinnedDragScrollStep(false, 4, 4, 20, 100, 6)).toBe(null);
    expect(pinnedDragScrollStep(true, 3, 4, 20, 100, 6)).toBe(null);
  });
});

describe("pinnedDragSnapshotIsValid", () => {
  it("pinned_drag_snapshot_requires_every_original_pin", () => {
    const snapshot = ["a", "b"];
    expect(pinnedDragSnapshotIsValid("a", snapshot, new Set(["new", "a", "b"]))).toBe(true);
    expect(pinnedDragSnapshotIsValid("a", snapshot, new Set(["a"]))).toBe(false);
  });
});

describe("sidebarPinProfileKey", () => {
  it("sidebar_pin_profile_keys_include_the_full_workspace_identity", () => {
    expect(sidebarPinProfileKey("local", null)).toBe("local");
    expect(sidebarPinProfileKey("synced", "device-1")).toBe("synced:device-1");
    expect(sidebarPinProfileKey("development", "device-2")).toBe("development:device-2");
  });

  it("sidebar_pin_profile_key_waits_for_engine_info", () => {
    // No scope yet (the engine's first frame has not landed): no key.
    expect(sidebarPinProfileKey(null, "device-1")).toBe(null);
    // A non-local scope without the engine's device id is equally unready.
    expect(sidebarPinProfileKey("synced", null)).toBe(null);
    expect(sidebarPinProfileKey("development", null)).toBe(null);
    // A local profile needs no device id, exactly like the desktop.
    expect(sidebarPinProfileKey("local", null)).toBe("local");
  });
});

describe("pinned disclosure geometry (38a8f013)", () => {
  it("pinned_section_body_height is inset + rows + gaps — the tween's target", () => {
    // The Rust window test's fixture: two 61px rows → 4 + 61 + 2 + 61 = 128.
    expect(pinnedSectionBodyHeight([61, 61])).toBe(128);
    expect(pinnedSectionBodyHeight([61])).toBe(65);
    expect(pinnedSectionBodyHeight([])).toBe(4);
  });

  it("the header's keyed height carries the open body's inset, minus one gap", () => {
    // 28 closed; 28 + (4 - 2) open — the phantom entry that keeps the rows
    // below accounting for the section (`render_chat_sidebar`'s order vec).
    expect(pinnedHeaderKeyedHeight(true)).toBe(30);
    expect(pinnedHeaderKeyedHeight(false)).toBe(28);
  });

  it("pinned_drag_accounts_for_disclosure_header_and_scroll: slot 0 sits 36px down", () => {
    // The desktop's drag y is viewport-relative and subtracts the 4px list
    // padding, the 28px header, and the 4px body inset, so the first row is
    // 36px below the viewport top; the web arms drags off the rows group's
    // own rect, which lands the same 36px in. The group-relative probe of
    // the desktop's fixture:
    const viewportTop = 100;
    const firstRowTop = 136;
    const rel = (pointerY: number): number => pointerY - viewportTop - 36;
    expect(pinnedSessionDropIndex(rel(firstRowTop), 3)).toBe(0);
    // A pointer 11px into the section (still inside the header) is no drop.
    expect(pinnedSessionDropIndex(rel(125), 3)).toBe(null);
    // A 63px scroll displacement moves the pointer one slot down the rows.
    expect(pinnedSessionDropIndex(rel(firstRowTop) + 63, 3)).toBe(1);
    // The divider frame is the hairline box plus its 2px top gap.
    expect(SIDEBAR_PINNED_DIVIDER_FRAME_HEIGHT).toBe(15);
  });
});

describe("commitVisiblePinReorder", () => {
  it("a single bucket reorders exactly like the desktop (hidden pins hold their slots)", () => {
    const buckets = { local: ["a1", "b1", "a2", "archived", "b2"] };
    // The visible projection of that bucket in display order; drag a1 onto a2's slot.
    expect(commitVisiblePinReorder(buckets, ["a1", "b1", "a2", "b2"], 0, 2)).toEqual({
      local: ["b1", "a2", "a1", "archived", "b2"],
    });
  });

  it("a within-bucket drag reorders it while other buckets stay untouched", () => {
    const buckets = {
      local: ["a1", "a2"],
      "synced:device-1": ["s1"],
    };
    // Merged visible projection [a1, a2, s1]; drag a2 to the top.
    const next = commitVisiblePinReorder(buckets, ["a1", "a2", "s1"], 1, 0);
    expect(next).toEqual({ local: ["a2", "a1"], "synced:device-1": ["s1"] });
  });

  it("membership never crosses buckets: a cross-bucket drag settles back into blocks", () => {
    const buckets = {
      local: ["a1", "a2"],
      "synced:device-1": ["s1"],
    };
    // Drag s1 to the top of the merged projection — ids never change buckets,
    // so the re-projection reads as the same bucket blocks.
    expect(commitVisiblePinReorder(buckets, ["a1", "a2", "s1"], 2, 0)).toEqual({
      local: ["a1", "a2"],
      "synced:device-1": ["s1"],
    });
  });
});

describe("sidebarSessionDropPins", () => {
  it("session_transfers_only_change_pin_membership_and_order", () => {
    const saved = ["hidden", "a", "b", "hidden-tail"];
    const visible = ["a", "b"];
    // A regular drop on an unpinned chat is a no-op.
    expect(sidebarSessionDropPins(saved, visible, "normal", { kind: "regular" })).toEqual(saved);
    // A pinned drop inserts before the visible anchor at the index.
    expect(sidebarSessionDropPins(saved, visible, "normal", { kind: "pinned", index: 1 })).toEqual([
      "hidden",
      "a",
      "normal",
      "b",
      "hidden-tail",
    ]);
    expect(sidebarSessionDropPins(saved, visible, "normal", { kind: "pinned", index: 2 })).toEqual([
      "hidden",
      "a",
      "b",
      "normal",
      "hidden-tail",
    ]);
    // Unpinning removes; a pinned drop of a pinned id reorders it.
    expect(sidebarSessionDropPins(saved, visible, "a", { kind: "regular" })).toEqual([
      "hidden",
      "b",
      "hidden-tail",
    ]);
    expect(sidebarSessionDropPins(saved, visible, "a", { kind: "pinned", index: 1 })).toEqual([
      "hidden",
      "b",
      "a",
      "hidden-tail",
    ]);
    // The first pin lands alone; the last unpin empties the list.
    expect(sidebarSessionDropPins([], [], "first", { kind: "pinned", index: 0 })).toEqual([
      "first",
    ]);
    expect(sidebarSessionDropPins(["only"], ["only"], "only", { kind: "regular" })).toEqual([]);
  });
});

describe("sidebarGapOffset", () => {
  it("transfer_gaps_shift_neighbors_without_reordering_data", () => {
    // Entering from another section opens a full slot at the destination.
    expect(sidebarGapOffset(0, null, 1, 63)).toBe(0);
    expect(sidebarGapOffset(1, null, 1, 63)).toBe(63);
    expect(sidebarGapOffset(2, null, 1, 63)).toBe(63);
    // Within a normal group, its original vacant slot is reused.
    expect(sidebarGapOffset(0, 2, 0, 63)).toBe(63);
    expect(sidebarGapOffset(1, 2, 0, 63)).toBe(63);
    expect(sidebarGapOffset(2, 2, 0, 63)).toBe(0);
    expect(sidebarGapOffset(1, 0, 3, 63)).toBe(-63);
    expect(sidebarGapOffset(3, 0, 3, 63)).toBe(0);
  });
});

describe("commitSessionDrop", () => {
  it("an unpin settles the chat out of its bucket; other buckets untouched", () => {
    const buckets = {
      local: ["a", "b"],
      "synced:device-1": ["s1"],
    };
    expect(commitSessionDrop(buckets, ["a", "b"], "a", { kind: "regular" })).toEqual({
      local: ["b"],
      "synced:device-1": ["s1"],
    });
  });

  it("a pin lands in the owning bucket, or the first bucket for a new pin", () => {
    // The visible pin projection is the merged bucket order; "r1" is a
    // regular chat being pinned at the top.
    expect(
      commitSessionDrop({ local: ["a"], "synced:device-1": ["s1"] }, ["a", "s1"], "r1", {
        kind: "pinned",
        index: 0,
      }),
    ).toEqual({ local: ["r1", "a"], "synced:device-1": ["s1"] });
    // A chat with no bucket yet pins into the first bucket (the web's
    // pin-in path builds one bucket per registry engine).
    expect(commitSessionDrop({ local: [] }, [], "r1", { kind: "pinned", index: 0 })).toEqual({
      local: ["r1"],
    });
  });
});

describe("pinOrderedRows", () => {
  it("render_active_rows's pin split: pins lead in saved order, regulars keep theirs", () => {
    const rows = chatRows(["newest", "p2", "middle", "p1", "oldest"]);
    const { pinned, regular } = pinOrderedRows(rows, ["p1", "p2", "archived"]);
    expect(pinned.map((row) => row.chat.id)).toEqual(["p1", "p2"]);
    expect(regular.map((row) => row.chat.id)).toEqual(["newest", "middle", "oldest"]);
  });

  it("no pins leaves the rows untouched", () => {
    const rows = chatRows(["b", "a"]);
    const { pinned, regular } = pinOrderedRows(rows, []);
    expect(pinned).toEqual([]);
    expect(regular.map((row) => row.chat.id)).toEqual(["b", "a"]);
  });
});

// ---- helpers ----

function chatRows(ids: readonly string[]): ChatRow[] {
  return ids.map((id) => ({
    chat: {
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
    },
    status: "idle" as const,
    project: "~",
    projectPath: null,
    folder: "~",
    harness: null,
    branch: null,
    timeAgo: "now",
    deviceId: "device-1",
    deviceName: null,
    deviceOffline: false,
    changeRequest: null,
  }));
}
