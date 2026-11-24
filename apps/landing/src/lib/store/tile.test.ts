import { expect, test } from "vitest";
import { corner, hue, initial } from "./tile.ts";

test("a plugin's hue is steady per id, the app's sum", () => {
  // (n = (n * 31 + byte) % 9973 over the id's bytes, then the eight hues.)
  let n = 0;
  for (const byte of new TextEncoder().encode("focus-timer")) n = (n * 31 + byte) % 9973;
  expect(hue("focus-timer")).toBe(["indigo", "rose", "sky", "emerald", "amber", "violet", "orange", "teal"][n % 8]);
  expect(hue("focus-timer")).toBe(hue("focus-timer"));
});

test("an initial is the name's first character, whole", () => {
  expect(initial("quick links")).toBe("Q");
  expect(initial("  émoji")).toBe("É");
  expect(initial("👩‍💻 dev")).toBe("👩‍💻");
  expect(initial("")).toBe("?");
});

test("corners grow with the tile", () => {
  expect([corner(18), corner(32), corner(40), corner(64)]).toEqual(["sm", "md", "md", "xl"]);
});
