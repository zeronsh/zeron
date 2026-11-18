import { describe, expect, test } from "vitest";
import { drawingChecks } from "./drawing.ts";
import type { Why } from "./rendered.ts";

const as = (cut: Why | null) => ({ html: "", cut });

describe("a reviewer is told when a README or notes are shown as text, whatever the cause", () => {
  test.each([
    ["size", "This README is too long to lay out", "Its notes for 1.2.0 are too long to lay out"],
    ["keep", "This README is too long to lay out", "Its notes for 1.2.0 are too long to lay out"],
    ["error", "This README could not be drawn", "Its notes for 1.2.0 could not be drawn"],
    ["attempt", "This README could not be drawn", "Its notes for 1.2.0 could not be drawn"],
  ] as const)("%s: a failing row each, in the danger ink", (cut, readme, notes) => {
    const rows = drawingChecks(as(cut), as(cut), "1.2.0");
    expect(rows.map((r) => [r.id, r.level])).toEqual([
      ["readme_drawn", "fail"],
      ["notes_drawn", "fail"],
    ]);
    expect(rows[0].message).toContain(readme);
    expect(rows[0].message).toContain("That alone is a reason not to approve it.");
    expect(rows[1].message).toContain(notes);
  });

  test("drawn, or not there: no row", () => {
    expect(drawingChecks(as(null), null, "1.2.0")).toEqual([]);
    expect(drawingChecks(null, as(null), "1.2.0")).toEqual([]);
  });
});
