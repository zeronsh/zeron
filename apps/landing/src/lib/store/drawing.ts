// What a reviewer is told when a submission's README or notes are shown as
// their text instead of drawn, whatever the cause: a row in the danger ink
// on every view (Overview, Files, Changes), counted with the rest in the
// rail and in Verify's line. A document that can't be drawn is itself a
// reason not to verify it.

import type { Drawn, Why } from "./rendered.ts";
import type { Check } from "./types.ts";

/** "too long to lay out" (past what it may write or keep), or "could not be drawn" (it failed, or a try at it never finished). */
const long = (why: Why) => why === "size" || why === "keep";

/** The rows for a submission's README and its notes for `version`: none when both were drawn. */
export function drawingChecks(readme: Drawn | null, notes: Drawn | null, version: string): Check[] {
  const rows: Check[] = [];
  if (readme?.cut) {
    rows.push({
      id: "readme_drawn",
      level: "fail",
      message: long(readme.cut)
        ? "This README is too long to lay out: it's shown as plain text. That alone is a reason not to verify it."
        : "This README could not be drawn: it's shown as plain text. That alone is a reason not to verify it.",
    });
  }
  if (notes?.cut) {
    rows.push({
      id: "notes_drawn",
      level: "fail",
      message: long(notes.cut) ? `Its notes for ${version} are too long to lay out: they're shown as plain text.` : `Its notes for ${version} could not be drawn: they're shown as plain text.`,
    });
  }
  return rows;
}
