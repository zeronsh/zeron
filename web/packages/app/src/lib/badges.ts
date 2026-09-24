/**
 * Message badges — `crates/ui/src/badges.rs` + the one extractor,
 * `crates/ui/src/comments.rs::extract_badge` (:158-238), ported 1:1.
 *
 * A feature stages context on the composer, folds it into the prompt as plain
 * text, and registers an [`Extractor`]. [`splitBadges`] lifts that block back
 * out of the sent message so the transcript draws a pill instead of the
 * bullets the agent reads. Nothing here knows what a diff comment is.
 */

import type { IconName } from "@zeron/icons";

/** A pill standing in for structured context a user message carries. */
export interface MessageBadge {
  /** `icons::*` asset name (`chatRoundLine` for the comment badge). */
  readonly icon: IconName;
  readonly label: string;
  /** Empty means the label says everything and the pill carries no card. */
  readonly details: readonly BadgeDetail[];
}

/** One row of a hover card — three generic slots (badges.rs:26-34). */
export interface BadgeDetail {
  /** `src/main.rs:42`, a URL, a commit subject. */
  readonly location: string;
  /** `L`/`R` for a diff side, a status, a count. */
  readonly tag: string | null;
  readonly body: string;
}

/**
 * Returns the text with this feature's block removed, plus the pill replacing
 * it. `null` when the message carries nothing of that kind (badges.rs:38).
 */
export type Extractor = (text: string) => { readonly text: string; readonly badge: MessageBadge } | null;

/** `COMMENT_BLOCK_HEADER` (comments.rs:123) — every comment is a diff comment. */
export const COMMENT_BLOCK_HEADER =
  "Comments on the diff (each cites the file and line it belongs to; L = line number in the original file, R = in the changed file):";
/** `REVIEW_COMMENT_BLOCK_HEADER` (comments.rs:124-125) — any file comment rides this one. */
export const REVIEW_COMMENT_BLOCK_HEADER =
  "Review comments (each cites the workspace file and line it belongs to):";
/** The stand-in body a comment-only send carries (comments.rs:121). */
export const COMMENT_ONLY_TEXT = "Address the review comments below.";

/**
 * Each extractor sees what the previous ones left behind, so two features can
 * ride the same prompt (badges.rs:44-54). The array is the extension point —
 * a future extractor just appends.
 */
export function splitBadges(text: string): { readonly text: string; readonly badges: readonly MessageBadge[] } {
  let rest = text;
  const badges: MessageBadge[] = [];
  for (const extract of EXTRACTORS) {
    const out = extract(rest);
    if (out !== null) {
      rest = out.text;
      badges.push(out.badge);
    }
  }
  return { text: rest, badges };
}

/**
 * `comments::extract_badge` (comments.rs:158-189): the comment block, matched
 * only as a whole TRAILING block, so a prompt quoting the header mid-body is
 * left alone. Accepts either header; the LAST occurrence wins.
 */
export function extractCommentBadge(text: string): { readonly text: string; readonly badge: MessageBadge } | null {
  const markers = [`\n\n${COMMENT_BLOCK_HEADER}\n`, `\n\n${REVIEW_COMMENT_BLOCK_HEADER}\n`];
  let at = -1;
  let marker = "";
  for (const candidate of markers) {
    const found = text.lastIndexOf(candidate);
    if (found > at) {
      at = found;
      marker = candidate;
    }
  }
  if (at < 0) {
    return null;
  }
  const block = text.slice(at + marker.length);
  if (
    block.length === 0 ||
    !block
      .split("\n")
      .every((line) => line.startsWith("- ") || line.startsWith("  "))
  ) {
    return null;
  }
  const details = parseBullets(block);
  if (details.length === 0) {
    return null;
  }
  return {
    text: text.slice(0, at),
    badge: {
      icon: "chatRoundLine",
      label: chipLabel(details.length),
      details,
    },
  };
}

/** The extractor registry (badges.rs:40) — one entry today. */
const EXTRACTORS: readonly Extractor[] = [extractCommentBadge];

/**
 * `parse_bullets` (comments.rs:190-238): one `BadgeDetail` per `"- "` bullet;
 * a `"  "` continuation line appends to the previous detail's body (that is
 * how a multi-line comment survives the round trip). For a bullet, two
 * candidate splits are computed and the EARLIEST wins — a body may itself
 * contain `"(L): "`, and matching that would swallow the body into the
 * location.
 */
export function parseBullets(block: string): BadgeDetail[] {
  const details: BadgeDetail[] = [];
  for (const line of block.split("\n")) {
    if (!line.startsWith("- ")) {
      const indented = line.startsWith("  ") ? line.slice(2) : null;
      const last = details[details.length - 1];
      if (indented !== null && last !== undefined) {
        details[details.length - 1] = { ...last, body: `${last.body}\n${indented}` };
      }
      continue;
    }
    const bullet = line.slice(2);
    // Side markers `" (L): "` / `" (R): "` (comments.rs:13-19, :127-129).
    let side: { at: number; markerLen: number; tag: string } | null = null;
    for (const tag of ["L", "R"]) {
      const marker = ` (${tag}): `;
      const at = bullet.indexOf(marker);
      if (at >= 0 && (side === null || at < side.at)) {
        side = { at, markerLen: marker.length, tag };
      }
    }
    const fileSplit = splitFileBullet(bullet);
    if (fileSplit !== null && (side === null || fileSplit.at < side.at)) {
      details.push({ location: fileSplit.location, tag: null, body: fileSplit.body });
      continue;
    }
    if (side !== null) {
      details.push({
        location: bullet.slice(0, side.at),
        tag: side.tag,
        body: bullet.slice(side.at + side.markerLen),
      });
      continue;
    }
    if (fileSplit !== null) {
      details.push({ location: fileSplit.location, tag: null, body: fileSplit.body });
    }
  }
  return details;
}

/**
 * `split_file_bullet` (comments.rs:239-246): the first `": "` whose preceding
 * text ends in `:{digits}` — i.e. `"{path}:{line}"` parses as a line number.
 */
function splitFileBullet(bullet: string): { at: number; location: string; body: string } | null {
  let search = 0;
  for (;;) {
    const at = bullet.indexOf(": ", search);
    if (at < 0) {
      return null;
    }
    search = at + 1;
    const location = bullet.slice(0, at);
    const colon = location.lastIndexOf(":");
    if (colon < 0) {
      continue;
    }
    const line = location.slice(colon + 1);
    if (/^\d+$/.test(line)) {
      return { at, location, body: bullet.slice(at + 2) };
    }
  }
}

/** `chip_label` (comments.rs:247-253). */
export function chipLabel(count: number): string {
  return count === 1 ? "1 comment" : `${count} comments`;
}
