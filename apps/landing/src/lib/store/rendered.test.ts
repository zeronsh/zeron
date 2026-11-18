import { expect, test } from "vitest";
import { Budget, PIPELINE, render as renderWithin, RENDERER, type Rendered, type RenderOptions } from "./markdown.ts";
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
  const render = (md: string, o: RenderOptions, b?: Budget): Rendered => (renders++, renderWithin(md, o, b));
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

test("two plugins with one README text each draw their own pictures, whoever's is kept first", async () => {
  const store = memory();
  const readme = "# Board\n\n![Board](shots/board.png)\n";
  const own = (id: string, version: string) => ({ file: (p: string) => `/plugins/${id}/files/${version}/${p}`, demote: true, title: "Board" });
  // (The copy is kept first; the victim's page still draws from the victim's package.)
  const copy = await renderedOnce("https://zeron.sh", "copycat", "1.0.0", "readme", readme, own("copycat", "1.0.0"), { store });
  const victim = await renderedOnce("https://zeron.sh", "victim", "1.0.0", "readme", readme, own("victim", "1.0.0"), { store });
  const later = await renderedOnce("https://zeron.sh", "victim", "1.1.0", "readme", readme, own("victim", "1.1.0"), { store });
  expect(copy).toContain('src="/plugins/copycat/files/1.0.0/shots/board.png"');
  expect(victim).toContain('src="/plugins/victim/files/1.0.0/shots/board.png"');
  expect(victim).not.toContain("copycat");
  expect(later).toContain('src="/plugins/victim/files/1.1.0/shots/board.png"');
  // (The page's own options are in the key too: the same text and pictures with another title or level is another rendering.)
  const titled = await renderedOnce("https://zeron.sh", "victim", "1.0.0", "readme", readme, { ...own("victim", "1.0.0"), title: "Other" }, { store });
  const level = await renderedOnce("https://zeron.sh", "victim", "1.0.0", "readme", readme, { ...own("victim", "1.0.0"), title: "Other", demote: false }, { store });
  expect(victim).not.toContain("<h");
  expect(titled).toContain("<h2");
  expect(level).toContain("<h1");
  expect(store.size()).toBe(5);
});

test("a change to the pipeline or the sanitizer's schema misses every kept rendering", async () => {
  const store = memory();
  let renders = 0;
  const render = (md: string, o: RenderOptions, b?: Budget): Rendered => (renders++, renderWithin(md, o, b));
  const options = { file: (p: string) => `/plugins/a/files/1.0.0/${p}`, demote: true };
  await renderedOnce("https://zeron.sh", "a", "1.0.0", "readme", "# Hi", options, { store, render });
  // (RENDERER bumped by hand, or the schema's fingerprint changed by itself.)
  await renderedOnce("https://zeron.sh", "a", "1.0.0", "readme", "# Hi", options, { store, render, pipeline: `${RENDERER + 1}.${PIPELINE.split(".").slice(1).join(".")}` });
  await renderedOnce("https://zeron.sh", "a", "1.0.0", "readme", "# Hi", options, { store, render, pipeline: `${RENDERER}.another-schema.${PIPELINE.split(".").at(-1)}` });
  expect(renders).toBe(3);
  expect(store.size()).toBe(3);
  expect(PIPELINE.startsWith(`${RENDERER}.`)).toBe(true);
});

test("a document shown as text only because the page spent its budget isn't kept; one drawn with the budget is", async () => {
  const store = memory();
  const options = { file: (p: string) => `/plugins/a/files/1.0.0/${p}`, demote: true };
  const spent = new Budget(0);
  const html = await renderedOnce("https://zeron.sh", "a", "1.0.0", "notes", "Some *notes*.", options, { store, budget: spent });
  expect(html).toContain("Shown as plain text: it's too long or too deeply nested to lay out.");
  expect(store.size()).toBe(0);
  const drawn = await renderedOnce("https://zeron.sh", "a", "1.0.0", "notes", "Some *notes*.", options, { store, budget: new Budget() });
  expect(drawn).toContain("<em>notes</em>");
  expect(store.size()).toBe(1);
});

