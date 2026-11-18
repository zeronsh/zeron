import { expect, test } from "vitest";
import { PIPELINE, renderMarkdown, RENDERER } from "./markdown.ts";
import { renderedOnce, type Keeper } from "./rendered.ts";

/** A cache in memory, as the Worker's keeps responses. */
const memory = (): Keeper & { size(): number } => {
  const kept = new Map<string, string>();
  return {
    match: async (r) => (kept.has(r.url) ? new Response(kept.get(r.url)) : undefined),
    put: async (r, response) => void kept.set(r.url, await response.text()),
    size: () => kept.size,
  };
};

test("a rendering is kept, and found again for the same text", async () => {
  const store = memory();
  let renders = 0;
  const render: typeof renderMarkdown = (md, o) => (renders++, renderMarkdown(md, o));
  const options = { file: (p: string) => `/plugins/a/files/1.0.0/${p}`, demote: true };
  const first = await renderedOnce("https://zeron.sh", "a", "1.0.0", "readme", "# Hi", options, { store, render });
  const again = await renderedOnce("https://zeron.sh", "a", "1.0.0", "readme", "# Hi", options, { store, render });
  expect(again).toBe(first);
  expect(renders).toBe(1);
  // Other text, another site (staging), or other pictures: never the same rendering.
  await renderedOnce("https://zeron.sh", "a", "1.0.0", "readme", "# Hello", options, { store, render });
  await renderedOnce("https://another-origin.example", "a", "1.0.0", "readme", "# Hi", options, { store, render });
  expect(renders).toBe(3);
});

test("a change to the pipeline or the sanitizer's schema misses every kept rendering", async () => {
  const store = memory();
  let renders = 0;
  const render: typeof renderMarkdown = (md, o) => (renders++, renderMarkdown(md, o));
  const options = { file: (p: string) => `/plugins/a/files/1.0.0/${p}`, demote: true };
  await renderedOnce("https://zeron.sh", "a", "1.0.0", "readme", "# Hi", options, { store, render });
  // (RENDERER bumped by hand, or the schema's fingerprint changed by itself.)
  await renderedOnce("https://zeron.sh", "a", "1.0.0", "readme", "# Hi", options, { store, render, pipeline: `${RENDERER + 1}.${PIPELINE.split(".").slice(1).join(".")}` });
  await renderedOnce("https://zeron.sh", "a", "1.0.0", "readme", "# Hi", options, { store, render, pipeline: `${RENDERER}.another-schema.${PIPELINE.split(".").at(-1)}` });
  expect(renders).toBe(3);
  expect(store.size()).toBe(3);
  expect(PIPELINE.startsWith(`${RENDERER}.`)).toBe(true);
});
