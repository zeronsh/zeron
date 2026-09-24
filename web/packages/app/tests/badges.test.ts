import { describe, expect, it } from "vitest";
import { MARK_CELLS, MARK_SPREAD, MINI_RING, MINI_RING_LEN, markCellStagger } from "../src/components/glyph-spinner";
import {
  COMMENT_BLOCK_HEADER,
  COMMENT_ONLY_TEXT,
  REVIEW_COMMENT_BLOCK_HEADER,
  chipLabel,
  extractCommentBadge,
  parseBullets,
  splitBadges,
} from "../src/lib/badges";

/**
 * The composer's writer shape (comments.rs:131-157 `with_comments`), inlined
 * per fixture: `{body}\n\n{header}\n{bullets}` — the extractor must read this
 * exact block back out. The writer itself is ticket 23's.
 */
function withCommentBlock(body: string, bullets: string[], fileComment = false): string {
  const header = fileComment ? REVIEW_COMMENT_BLOCK_HEADER : COMMENT_BLOCK_HEADER;
  return `${body}\n\n${header}\n${bullets.join("\n")}`;
}

describe("badges pure logic", () => {
  it("a_plain_message_carries_no_badges", () => {
    const out = splitBadges("just a prompt");
    expect(out.text).toBe("just a prompt");
    expect(out.badges).toEqual([]);
  });

  it("a_sent_comment_block_becomes_one_pill", () => {
    const staged = withCommentBlock("do", [
      "- a.rs:3 (R): fix",
      "- b.rs:9 (L): why",
    ]);
    const out = splitBadges(staged);
    expect(out.text).toBe("do");
    expect(out.badges.length).toBe(1);
    expect(out.badges[0]!.label).toBe("2 comments");
    expect(out.badges[0]!.icon).toBe("chatRoundLine");
  });

  it("the_card_carries_one_row_per_comment", () => {
    const staged = withCommentBlock("look", [
      "- src/main.rs:42 (R): early-return here",
      "- src/lib.rs:7 (L): why was this dropped?",
    ]);
    const { badges } = splitBadges(staged);
    const details = badges[0]!.details;
    expect(details.length).toBe(2);
    expect(details[0]!.location).toBe("src/main.rs:42");
    expect(details[0]!.tag).toBe("R");
    expect(details[0]!.body).toBe("early-return here");
    expect(details[1]!.location).toBe("src/lib.rs:7");
    expect(details[1]!.tag).toBe("L");
  });

  it("a_multiline_body_rejoins_its_continuation_lines", () => {
    const staged = withCommentBlock("x", ["- a.rs:3 (R): first\n  second"]);
    const { badges } = splitBadges(staged);
    expect(badges[0]!.details[0]!.body).toBe("first\nsecond");
  });

  it("a_path_with_a_colon_still_splits_on_the_side_marker", () => {
    const staged = withCommentBlock("x", ["- odd:name.rs:5 (R): hm"]);
    const { badges } = splitBadges(staged);
    expect(badges[0]!.details[0]!.location).toBe("odd:name.rs:5");
    expect(badges[0]!.details[0]!.body).toBe("hm");
  });

  it("a_comment_only_send_keeps_its_stand_in_body", () => {
    // `with_comments("", …)` substitutes COMMENT_ONLY_TEXT as the body.
    const staged = withCommentBlock(COMMENT_ONLY_TEXT, ["- a.rs:3 (R): fix"]);
    const out = splitBadges(staged);
    expect(out.text).toBe(COMMENT_ONLY_TEXT);
    expect(out.badges[0]!.label).toBe("1 comment");
  });

  it("file_comments_carry_no_side_tag_and_use_the_review_header", () => {
    // A file comment rides REVIEW_COMMENT_BLOCK_HEADER with a plain
    // `{path}:{line}: body` bullet (comments.rs:317-336).
    const staged = withCommentBlock(
      "review this",
      ["- crates/ui/src/files/preview.rs:1494: Keep this branch explicit"],
      true,
    );
    const out = splitBadges(staged);
    expect(out.text).toBe("review this");
    expect(out.badges[0]!.details[0]!.location).toBe("crates/ui/src/files/preview.rs:1494");
    expect(out.badges[0]!.details[0]!.tag).toBeNull();
    expect(out.badges[0]!.details[0]!.body).toBe("Keep this branch explicit");
  });

  it("a_body_quoting_a_side_marker_survives_the_round_trip", () => {
    // Earliest-marker-wins is load-bearing (comments.rs:201-203): the body's
    // own "(L): " must not swallow the body into the location.
    const staged = withCommentBlock("x", ["- a.rs:5 (R): see (L): the other one"]);
    const { badges } = splitBadges(staged);
    expect(badges[0]!.details.length).toBe(1);
    expect(badges[0]!.details[0]!.location).toBe("a.rs:5");
    expect(badges[0]!.details[0]!.tag).toBe("R");
    expect(badges[0]!.details[0]!.body).toBe("see (L): the other one");
  });

  it("a_mid_body_quote_of_the_header_is_left_alone", () => {
    // The block matches only as a whole TRAILING block (comments.rs:157).
    const quoting = `Before\n\n${COMMENT_BLOCK_HEADER}\n- a.rs:1 (R): one\n\nafter the block`;
    expect(extractCommentBadge(quoting)).toBeNull();
    // A block whose lines are not bullets is left alone too.
    const notBullets = `x\n\n${COMMENT_BLOCK_HEADER}\nplain prose line`;
    expect(extractCommentBadge(notBullets)).toBeNull();
  });

  it("the_last_block_wins_and_either_header_is_accepted", () => {
    const twice = `first\n\n${COMMENT_BLOCK_HEADER}\n- a.rs:1 (R): one\n\nagain\n\n${REVIEW_COMMENT_BLOCK_HEADER}\n- b.rs:2 (L): two`;
    const out = splitBadges(twice);
    expect(out.text).toBe("first\n\nComments on the diff (each cites the file and line it belongs to; L = line number in the original file, R = in the changed file):\n- a.rs:1 (R): one\n\nagain");
    expect(out.badges[0]!.details[0]!.location).toBe("b.rs:2");
  });

  it("chip_label_pluralizes", () => {
    expect(chipLabel(1)).toBe("1 comment");
    expect(chipLabel(2)).toBe("2 comments");
    expect(chipLabel(0)).toBe("0 comments");
  });

  it("parse_bullets_skips_continuations_before_any_bullet", () => {
    // A continuation with no preceding bullet is dropped (comments.rs:196-199).
    expect(parseBullets("  orphan")).toEqual([]);
  });

  it("the_mini_ring_visits_every_cell_once", () => {
    // The loader table (proto/motion.rs:38-43, test :582-586): the ring is a
    // permutation of 0..6 — the chase covers every cell exactly once.
    const seen = [...MINI_RING.flat()].sort((a, b) => a - b);
    expect(seen).toEqual([...Array(MINI_RING_LEN).keys()]);
  });

  it("mark_stagger_follows_flight_axis", () => {
    // The mark loader's sweep (loaders.rs:389-405): the tail tip leads, the
    // head anchors, every cell inside the spread window — the geometry the
    // `ZeronMarkLoader` fixture animates.
    const tail = markCellStagger(720, 0);
    const head = markCellStagger(0, 840);
    expect(tail).toBeGreaterThan(head);
    expect(head).toBeCloseTo(0);
    expect(tail).toBeLessThanOrEqual(MARK_SPREAD + 1e-6);
    for (const [x, y] of MARK_CELLS) {
      const s = markCellStagger(x, y);
      expect(s).toBeGreaterThanOrEqual(0);
      expect(s).toBeLessThanOrEqual(MARK_SPREAD + 1e-6);
    }
  });
});
