import { describe, expect, it } from "vitest";
import { ATTACHMENT_ONLY_TEXT, withAttachments } from "../src/lib/attachments";
import {
  QUEUE_ROW_SLOT,
  availableQueuePrimaryAction,
  modifierSendCompactLabel,
  modifierSendLabel,
  oneLine,
  queueAttachmentLabels,
  queueAttachmentSummary,
  queueDragOffsets,
  queueDropIndex,
  queueLatestShortcutVisible,
  queuePreviewLimit,
  queueVisibleText,
  visibleQueueRows,
} from "../src/lib/queue-row-logic";

/**
 * The queue row's pure logic — the web mirrors of `crates/ui/src/queue.rs`'s
 * unit tests (each function named after the desktop test it mirrors), plus
 * `queue_preview_limit` (composer.rs:4392) and `modifier_send_label`
 * (settings/shortcuts.rs:994).
 */

describe("oneLine (queue.rs::rows_flatten_multi_line_messages)", () => {
  it("flattens a multi-line message to one visual line", () => {
    expect(oneLine("fix the test\n\nthen ship it")).toBe("fix the test then ship it");
  });

  it("collapses whitespace runs", () => {
    expect(oneLine("  spaced   out  ")).toBe("spaced out");
  });
});

describe("availableQueuePrimaryAction (queue.rs::available_primary_action_obeys_row_and_host_gates)", () => {
  it("is Send now only when the row is not blocked and the host supports actions", () => {
    expect(availableQueuePrimaryAction(false, true)).toBe("sendNow");
    expect(availableQueuePrimaryAction(true, true)).toBeNull();
    expect(availableQueuePrimaryAction(false, false)).toBeNull();
  });
});

describe("queueLatestShortcutVisible (queue.rs::queue_shortcut_only_appears_on_the_actionable_latest_row_when_revealed)", () => {
  it("shows only on the actionable latest row when revealed", () => {
    expect(queueLatestShortcutVisible(0, 2, true, true)).toBe(false);
    expect(queueLatestShortcutVisible(1, 2, true, true)).toBe(true);
    expect(queueLatestShortcutVisible(1, 2, false, true)).toBe(false);
    expect(queueLatestShortcutVisible(1, 2, true, false)).toBe(false);
    // count == 0 can never match — no out-of-range index.
    expect(queueLatestShortcutVisible(0, 0, true, true)).toBe(false);
  });
});

describe("queueDropIndex (queue.rs::the_whole_panel_maps_to_a_clamped_queue_drop_slot)", () => {
  it("maps the whole panel to a clamped row slot", () => {
    expect(queueDropIndex(0, 2)).toBe(0);
    expect(queueDropIndex(QUEUE_ROW_SLOT - 0.1, 2)).toBe(0);
    expect(queueDropIndex(QUEUE_ROW_SLOT, 2)).toBe(1);
    expect(queueDropIndex(10_000, 2)).toBe(1);
  });
});

describe("queueDragOffsets (queue.rs::drag_offsets_move_the_real_row_and_open_its_destination)", () => {
  it("moves the real row and opens its destination", () => {
    expect(queueDragOffsets(0, 0, 0, 2)).toEqual([0, 2 * QUEUE_ROW_SLOT]);
    expect(queueDragOffsets(1, 0, 0, 2)).toEqual([0, -QUEUE_ROW_SLOT]);
    expect(queueDragOffsets(2, 0, 0, 2)).toEqual([0, -QUEUE_ROW_SLOT]);
  });

  it("restarts only the rows whose visual destination changed", () => {
    expect(queueDragOffsets(0, 0, 2, 1)).toEqual([2 * QUEUE_ROW_SLOT, QUEUE_ROW_SLOT]);
    expect(queueDragOffsets(1, 0, 2, 1)).toEqual([-QUEUE_ROW_SLOT, -QUEUE_ROW_SLOT]);
    expect(queueDragOffsets(2, 0, 2, 1)).toEqual([-QUEUE_ROW_SLOT, 0]);
  });
});

describe("visibleQueueRows (queue.rs::queue_preview_work_follows_the_visible_rows)", () => {
  it("windows the row range whose thumbnails are worth decoding", () => {
    // gpui's scroll offset runs negative downward.
    expect(visibleQueueRows(-QUEUE_ROW_SLOT * 20, QUEUE_ROW_SLOT * 3, 1000)).toEqual([20, 24]);
    expect(visibleQueueRows(-QUEUE_ROW_SLOT * 20, QUEUE_ROW_SLOT * 3, 0)).toEqual([0, 0]);
  });
});

describe("queueVisibleText (queue.rs::legacy_attachment_trailers_are_hidden_from_queue_text)", () => {
  const path = "/tmp/image.png";

  it("hides a matching legacy attachment trailer", () => {
    expect(queueVisibleText(withAttachments("inspect this", [path]), [path])).toBe("inspect this");
  });

  it("falls back to the attachment-only text", () => {
    expect(queueVisibleText(withAttachments("", [path]), [path])).toBe(ATTACHMENT_ONLY_TEXT);
    expect(queueVisibleText("", [path])).toBe(ATTACHMENT_ONLY_TEXT);
  });

  it("keeps literal user text when the trailer does not match", () => {
    expect(queueVisibleText("literal user text", [path])).toBe("literal user text");
    expect(queueVisibleText(withAttachments("inspect this", ["/tmp/other.png"]), [path])).toBe(
      withAttachments("inspect this", ["/tmp/other.png"]),
    );
  });
});

describe("queueAttachmentLabels (queue.rs::attachment_labels_…, appshot half dropped)", () => {
  it("labels every path with its bare filename", () => {
    expect(queueAttachmentLabels(["/tmp/shot & detail.png", "/tmp/reference.png"])).toEqual([
      "shot & detail.png",
      "reference.png",
    ]);
    expect(queueAttachmentLabels(["C:\\Users\\me\\pic.png"])).toEqual(["pic.png"]);
  });
});

describe("queueAttachmentSummary", () => {
  it("prefixes the count when there are several attachments", () => {
    expect(queueAttachmentSummary(["a.png", "b.png"])).toBe("2 attachments · a.png · b.png");
    expect(queueAttachmentSummary(["a.png"])).toBe("a.png");
    expect(queueAttachmentSummary([])).toBe("");
  });
});

describe("queuePreviewLimit (composer.rs:4392-4398)", () => {
  it("drops to one preview below 520px of composer width", () => {
    expect(queuePreviewLimit(519)).toBe(1);
    expect(queuePreviewLimit(520)).toBe(2);
    expect(queuePreviewLimit(null)).toBe(2);
    expect(queuePreviewLimit(768)).toBe(2);
  });
});

describe("modifierSendLabel (settings/shortcuts.rs::modifier_send_labels_are_platform_specific)", () => {
  it("spells the platform combo", () => {
    expect(modifierSendLabel(true)).toBe("⌘ Enter");
    expect(modifierSendLabel(false)).toBe("Ctrl Enter");
    expect(modifierSendCompactLabel(true)).toBe("⌘↵");
    expect(modifierSendCompactLabel(false)).toBe("⌃↵");
  });
});
