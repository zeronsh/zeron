import { expect, test } from "vitest";
import { PIPELINE, render as renderWithin, RENDERER, type Rendered, type RenderOptions } from "./markdown.ts";
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
  const render = (md: string, o: RenderOptions): Rendered => (renders++, renderWithin(md, o));
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

test("a change to the pipeline, the tags it may write or its bounds misses every kept rendering", async () => {
  const store = memory();
  let renders = 0;
  const render = (md: string, o: RenderOptions): Rendered => (renders++, renderWithin(md, o));
  const options = { file: (p: string) => `/plugins/a/files/1.0.0/${p}`, demote: true };
  await renderedOnce("https://zeron.sh", "a", "1.0.0", "readme", "# Hi", options, { store, render });
  // (RENDERER bumped by hand, or the tags' fingerprint changed by itself.)
  await renderedOnce("https://zeron.sh", "a", "1.0.0", "readme", "# Hi", options, { store, render, pipeline: `${RENDERER + 1}.${PIPELINE.split(".").slice(1).join(".")}` });
  await renderedOnce("https://zeron.sh", "a", "1.0.0", "readme", "# Hi", options, { store, render, pipeline: `${RENDERER}.other-tags.${PIPELINE.split(".").at(-1)}` });
  expect(renders).toBe(3);
  expect(store.size()).toBe(3);
  expect(PIPELINE.startsWith(`${RENDERER}.`)).toBe(true);
});

test("a document too large to draw is kept as its text, said so (the same text always comes out the same)", async () => {
  const store = memory();
  const options = { file: (p: string) => `/plugins/a/files/1.0.0/${p}`, demote: true };
  const wide = `|${"a|".repeat(64)}\n|${"-|".repeat(64)}\n${"a\n".repeat(20_000)}`;
  const html = await renderedOnce("https://zeron.sh", "a", "1.0.0", "notes", wide, options, { store });
  expect(html).toContain("Shown as plain text: it's too long to lay out.");
  expect(store.size()).toBe(1);
});

test("a key's fields can't run into each other: a title with a line end and the text after it are two renderings", async () => {
  const store = memory();
  let renders = 0;
  const render = (md: string, o: RenderOptions): Rendered => (renders++, renderWithin(md, o));
  const file = (p: string) => `/plugins/a/files/1.0.0/${p}`;
  // (Joined by line ends, these two were one key: "a\nb\nc".)
  await renderedOnce("https://zeron.sh", "a", "1.0.0", "readme", "b\nc", { file, title: "a" }, { store, render });
  await renderedOnce("https://zeron.sh", "a", "1.0.0", "readme", "c", { file, title: "a\nb" }, { store, render });
  expect(renders).toBe(2);
  expect(store.size()).toBe(2);
});
