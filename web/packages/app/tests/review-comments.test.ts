import { describe, expect, it } from "vitest";
import {
  COMMENT_BLOCK_HEADER,
  COMMENT_ONLY_TEXT,
  REVIEW_COMMENT_BLOCK_HEADER,
  cardBodyLines,
  cardHeight,
  citePath,
  editorCommentOverlayHorizontal,
  editorCommentOverlayTop,
  location,
  newDiffComment,
  newFileComment,
  splitBadges,
  withComments,
  type ReviewComment,
} from "../src/lib/review-comments";
import { diffAnchor, commentStripHeight } from "../src/lib/review-comments";

/**
 * The comments.rs test suite, ported 1:1 (comments.rs:279-421) plus the
 * ticket's §3 list. Every extract round-trip runs through the REAL writer
 * (`withComments`) — ticket 20's `badges.test.ts` inlines the block shape;
 * these prove the writer and the reader agree.
 */

function diffComment(path: string, side: "old" | "new", line: number, body: string): ReviewComment {
  return newDiffComment(path, side, line, body);
}

describe("review comments pure logic", () => {
  it("empty_set_leaves_the_prompt_untouched", () => {
    expect(withComments("ship it", [])).toBe("ship it");
  });

  it("comments_append_as_located_bullets", () => {
    const staged = [
      diffComment("src/main.rs", "new", 42, "early-return here"),
      diffComment("src/lib.rs", "old", 7, "why was this dropped?"),
    ];
    const out = withComments("look at these", staged);
    expect(out.startsWith("look at these\n\n")).toBe(true);
    expect(out).toContain("- src/main.rs:42 (R): early-return here");
    expect(out).toContain("- src/lib.rs:7 (L): why was this dropped?");
  });

  it("comment_only_send_gets_a_body", () => {
    const staged = [diffComment("a.rs", "new", 1, "fix")];
    expect(withComments("", staged).startsWith(COMMENT_ONLY_TEXT)).toBe(true);
  });

  it("multiline_bodies_indent_under_their_bullet", () => {
    const staged = [diffComment("a.rs", "new", 3, "first\nsecond")];
    expect(withComments("x", staged)).toContain("- a.rs:3 (R): first\n  second");
  });

  it("file_comments_use_current_workspace_locations_without_diff_tags", () => {
    const staged = [newFileComment("crates/ui/src/files/preview.rs", 1494, "Keep this branch explicit")];
    const out = withComments("review this", staged);
    expect(out).toContain(REVIEW_COMMENT_BLOCK_HEADER);
    expect(out).toContain("- crates/ui/src/files/preview.rs:1494: Keep this branch explicit");
    expect(out).not.toContain("(R)");
    const { text, badges } = splitBadges(out);
    expect(text).toBe("review this");
    expect(badges[0]!.details[0]!.location).toBe("crates/ui/src/files/preview.rs:1494");
    expect(badges[0]!.details[0]!.tag).toBeNull();
    expect(badges[0]!.details[0]!.body).toBe("Keep this branch explicit");
  });

  it("mixed_editor_and_diff_comments_share_one_review_block", () => {
    const staged = [
      diffComment("a.rs", "old", 3, "why removed?"),
      newFileComment("odd:path.rs", 8, "change this: please"),
    ];
    const { text, badges } = splitBadges(withComments("x", staged));
    expect(text).toBe("x");
    expect(badges[0]!.details.length).toBe(2);
    expect(badges[0]!.details[0]!.tag).toBe("L");
    expect(badges[0]!.details[1]!.location).toBe("odd:path.rs:8");
    expect(badges[0]!.details[1]!.body).toBe("change this: please");
  });

  it("file_comment_body_may_quote_a_diff_side_marker", () => {
    const staged = [newFileComment("a.rs", 2, "compare with (L): old code")];
    const { badges } = splitBadges(withComments("x", staged));
    expect(badges[0]!.details[0]!.location).toBe("a.rs:2");
    expect(badges[0]!.details[0]!.tag).toBeNull();
    expect(badges[0]!.details[0]!.body).toBe("compare with (L): old code");
  });

  it("a_body_quoting_a_side_marker_survives_the_round_trip", () => {
    // Earliest-marker-wins is load-bearing (comments.rs:201-203): the body's
    // own "(L): " must not swallow the body into the location.
    const staged = [diffComment("a.rs", "new", 5, "see (L): the other one")];
    const { text, badges } = splitBadges(withComments("x", staged));
    expect(text).toBe("x");
    expect(badges[0]!.details.length).toBe(1);
    expect(badges[0]!.details[0]!.location).toBe("a.rs:5");
    expect(badges[0]!.details[0]!.tag).toBe("R");
    expect(badges[0]!.details[0]!.body).toBe("see (L): the other one");
  });

  it("a_renamed_file_cites_the_side_the_line_lives_in", () => {
    const old = newDiffComment("new_name.rs", "old", 7, "why dropped?", "old_name.rs");
    const next = newDiffComment("new_name.rs", "new", 12, "nit", "old_name.rs");
    // The grouping key stays the diff's own path either way.
    expect(old.path).toBe("new_name.rs");
    expect(next.path).toBe("new_name.rs");
    const out = withComments("x", [old, next]);
    expect(out).toContain("- old_name.rs:7 (L): why dropped?");
    expect(out).toContain("- new_name.rs:12 (R): nit");
  });

  it("an_unrenamed_file_cites_its_only_path_on_both_sides", () => {
    const staged = [
      diffComment("a.rs", "old", 3, "gone"),
      diffComment("a.rs", "new", 4, "here"),
    ];
    const out = withComments("x", staged);
    expect(out).toContain("- a.rs:3 (L): gone");
    expect(out).toContain("- a.rs:4 (R): here");
  });

  // ── The ticket's six desktop badge test names (§3), through the writer ──

  it("a_plain_message_carries_no_badges", () => {
    const out = splitBadges("just a prompt");
    expect(out.text).toBe("just a prompt");
    expect(out.badges).toEqual([]);
  });

  it("a_sent_comment_block_becomes_one_pill", () => {
    const staged = [diffComment("a.rs", "new", 3, "fix"), diffComment("b.rs", "old", 9, "why")];
    const out = splitBadges(withComments("do", staged));
    expect(out.text).toBe("do");
    expect(out.badges.length).toBe(1);
    expect(out.badges[0]!.label).toBe("2 comments");
    expect(out.badges[0]!.icon).toBe("chatRoundLine");
  });

  it("the_card_carries_one_row_per_comment", () => {
    const staged = [
      diffComment("src/main.rs", "new", 42, "early-return here"),
      diffComment("src/lib.rs", "old", 7, "why was this dropped?"),
    ];
    const { badges } = splitBadges(withComments("look", staged));
    const details = badges[0]!.details;
    expect(details.length).toBe(2);
    expect(details[0]!.location).toBe("src/main.rs:42");
    expect(details[0]!.tag).toBe("R");
    expect(details[0]!.body).toBe("early-return here");
    expect(details[1]!.location).toBe("src/lib.rs:7");
    expect(details[1]!.tag).toBe("L");
  });

  it("a_multiline_body_rejoins_its_continuation_lines", () => {
    const staged = [diffComment("a.rs", "new", 3, "first\nsecond")];
    const { badges } = splitBadges(withComments("x", staged));
    expect(badges[0]!.details[0]!.body).toBe("first\nsecond");
  });

  it("a_path_with_a_colon_still_splits_on_the_side_marker", () => {
    const staged = [diffComment("odd:name.rs", "new", 5, "hm")];
    const { badges } = splitBadges(withComments("x", staged));
    expect(badges[0]!.details[0]!.location).toBe("odd:name.rs:5");
    expect(badges[0]!.details[0]!.body).toBe("hm");
  });

  it("a_comment_only_send_keeps_its_stand_in_body", () => {
    const staged = [diffComment("a.rs", "new", 3, "fix")];
    const out = splitBadges(withComments("", staged));
    expect(out.text).toBe(COMMENT_ONLY_TEXT);
    expect(out.badges[0]!.label).toBe("1 comment");
  });

  // ── Card geometry (comments.rs:410-421) ──────────────────────────────────

  it("card_height_grows_with_body_lines", () => {
    expect(cardHeight("two\nlines")).toBeGreaterThan(cardHeight("one"));
    expect(cardHeight("")).toBe(cardHeight("one"));
  });

  it("long_lines_are_charged_for_their_soft_wraps", () => {
    expect(cardBodyLines("short")).toBe(1);
    expect(cardBodyLines("x".repeat(64))).toBe(1);
    expect(cardBodyLines("x".repeat(65))).toBe(2);
    expect(cardBodyLines("line\n".repeat(200))).toBe(8);
  });

  // ── citePath / location / the anchor helpers ─────────────────────────────

  it("cite_path_and_location_follow_the_side_and_rename", () => {
    const old = newDiffComment("new_name.rs", "old", 7, "b", "old_name.rs");
    const next = newDiffComment("new_name.rs", "new", 12, "b", "old_name.rs");
    const file = newFileComment("notes.md", 3, "b");
    expect(citePath(old)).toBe("old_name.rs");
    expect(citePath(next)).toBe("new_name.rs");
    expect(citePath(file)).toBe("notes.md");
    expect(location(old)).toBe("old_name.rs:7");
    expect(location(file)).toBe("notes.md:3");
    expect(diffAnchor(old)).toEqual({ side: "old", line: 7 });
    expect(diffAnchor(file)).toBeNull();
  });

  // ── The composer strip budget (composer.rs:306-310) ─────────────────────

  it("comment_strip_height_is_arithmetic", () => {
    expect(commentStripHeight(0)).toBe(0);
    expect(commentStripHeight(1)).toBe(36);
    expect(commentStripHeight(4)).toBe(36);
  });

  // ── The editor overlay anchor math (preview.rs:3301-3326) ───────────────

  it("editor_overlay_horizontal_anchors_or_spans", () => {
    // A viewport with room: the anchored column at gutter − 8, capped 320.
    expect(editorCommentOverlayHorizontal(48, 1000)).toEqual({ left: 40, width: 320 });
    // Tight viewport: full width minus 8px margins on both sides.
    expect(editorCommentOverlayHorizontal(48, 200)).toEqual({ left: 8, width: 184 });
    // A gutter under the 8px margin floors the anchor at 8.
    expect(editorCommentOverlayHorizontal(4, 400)).toEqual({ left: 8, width: 320 });
  });

  it("editor_overlay_top_clamps_into_the_viewport", () => {
    expect(editorCommentOverlayTop(20, 20, 46, 800)).toBe(40);
    // A row near the bottom: the card clamps to the viewport's lower edge.
    expect(editorCommentOverlayTop(780, 20, 66, 800)).toBe(734);
    // A scrolled-away row (negative top) clamps to the top edge.
    expect(editorCommentOverlayTop(-40, 20, 66, 800)).toBe(0);
  });
});
