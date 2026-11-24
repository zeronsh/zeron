// What a rendering holds, on a real heap: the shapes that once ran a
// 128 MB heap out (the identity lane's F1 and F3: a definition used many
// times, rows a table must fill, a new list on every line, deep quotes, and
// a text of emphasis), each at the README's cap, in a child process whose
// heap is 48 MB, well under half a Worker's 128 (about 14 of it the
// highlighter's grammars, held whatever is drawn). One that can't fit
// kills the child, and this fails.

import { execFileSync } from "node:child_process";
import { fileURLToPath } from "node:url";
import { expect, test } from "vitest";

const HEAP_MB = 48;
const SHAPES = [
  "ref long dest& reused",
  "ref long title& reused",
  "ref long https dest reused",
  "img ref long title reused",
  "table wide 64 rows 1byte",
  "table wide 64 center rows",
  "lists alt markers",
  "list items empty",
  "quotes deep 40 + blank",
  "quote 32 then blank",
  "open then close",
  "*a runs",
  "nested images links",
  "an ordinary README",
];

test(`the heaviest shapes, at the README's cap, each fit a ${HEAP_MB} MB heap`, () => {
  const child = fileURLToPath(new URL("./heap-child.ts", import.meta.url));
  const stdout = execFileSync(process.execPath, [`--max-old-space-size=${HEAP_MB}`, "--expose-gc", "--experimental-strip-types", "--no-warnings", child, ...SHAPES], {
    encoding: "utf8",
    timeout: 120_000,
  });
  const results = JSON.parse(stdout) as { name: string; cut: boolean; retained: number }[];
  // (Each ran: none ran the heap out. And none left anything behind: after them all, an honest README's
  // render leaves what the first did, the grammars and little else.)
  expect(results.map((r) => r.name)).toEqual(SHAPES);
  const least = Math.min(...results.map((r) => r.retained));
  expect((results[results.length - 1].retained - least) / 1024 / 1024).toBeLessThan(4);
  // (An honest README at the cap is drawn; the hostile ones may be cut, said so.)
  expect(results.find((r) => r.name === "an ordinary README")!.cut).toBe(false);
}, 180_000);
