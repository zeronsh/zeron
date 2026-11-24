// Run by md/heap.test.ts in a child process under a small heap: renders each
// family named on the command line at the README's cap, and prints what the
// heap still holds after each once collected (a heap that can't hold one
// while it runs kills this process, so the test fails instead of the runner).

import { FAMILIES } from "./families.ts";
import { toHtml, TooLarge } from "./html.ts";
import { meter } from "./meter.ts";

const options = { file: (p: string) => `/plugins/a/files/1.0.0/${p}`, demote: true };
const out: { name: string; cut: boolean; retained: number; peak: number }[] = [];
const gc = (globalThis as { gc?: () => void }).gc;
for (const name of process.argv.slice(2)) {
  // (Each render on its own, as a Worker's requests are: what the last left behind is collected first.)
  gc?.();
  const md = FAMILIES[name](512 * 1024);
  const m = meter();
  let cut = false;
  try {
    toHtml(md, options, 2 * 1024 * 1024, m);
  } catch (error) {
    if (!(error instanceof TooLarge)) throw error;
    cut = true;
  }
  // (Twice: what one collection only marks, the next frees.)
  gc?.();
  gc?.();
  out.push({ name, cut, retained: process.memoryUsage().heapUsed, peak: m.peak });
}
process.stdout.write(JSON.stringify(out));
