import { describe, expect, test } from "vitest";
import { PIPELINE, render as renderWithin, RENDERER, type Rendered, type RenderOptions } from "./markdown.ts";
import { drawn, HOLDS, keepable, renderedOnce, type Keeper } from "./rendered.ts";

/** A cache in memory, as the Worker's keeps responses: each until its max-age, by a clock the test moves. */
const memory = (): Keeper & { size(): number; clock: { now: number } } => {
  const clock = { now: 0 };
  const kept = new Map<string, { body: string; headers: [string, string][]; until: number }>();
  const live = (url: string) => {
    const entry = kept.get(url);
    if (entry && entry.until <= clock.now) kept.delete(url);
    return kept.get(url);
  };
  return {
    clock,
    match: async (r) => {
      const entry = live(r.url);
      return entry ? new Response(entry.body, { headers: entry.headers }) : undefined;
    },
    put: async (r, response) => {
      const age = Number(/max-age=(\d+)/.exec(response.headers.get("cache-control") ?? "")?.[1] ?? 0);
      kept.set(r.url, { body: await response.text(), headers: [...response.headers], until: clock.now + age });
    },
    delete: async (r) => kept.delete(r.url),
    size: () => [...kept.keys()].filter((url) => live(url)).length,
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

describe("a try is recorded before it is made", () => {
  const options = { file: (p: string) => `/plugins/a/files/1.0.0/${p}`, demote: true };
  const at = (store: ReturnType<typeof memory>, seconds: number) => void (store.clock.now += seconds);

  test("a renderer that throws: the next request shows the text, said so, without trying; after the hold, one more try", async () => {
    const store = memory();
    let tries = 0;
    const dies = (): Rendered => {
      tries++;
      throw new Error("killed");
    };
    for (const keep of [true, false]) {
      const s = keep ? store : memory();
      tries = 0;
      const first = await drawn("https://zeron.sh", "a", "1.0.0", "readme", "# Hi <b>", options, { store: s, render: dies, keep });
      expect(first.cut).toBe("attempt");
      expect(first.html).toContain("Shown as plain text: it couldn't be laid out.");
      expect(first.html).toContain("# Hi &lt;b&gt;");
      const again = await drawn("https://zeron.sh", "a", "1.0.0", "readme", "# Hi <b>", options, { store: s, render: dies, keep });
      expect(again.cut).toBe("attempt");
      expect(tries).toBe(1);
      // (Held five minutes: just before, still held; then one more try.)
      at(s, HOLDS[0] - 1);
      await drawn("https://zeron.sh", "a", "1.0.0", "readme", "# Hi <b>", options, { store: s, render: dies, keep });
      expect(tries).toBe(1);
      at(s, 1);
      await drawn("https://zeron.sh", "a", "1.0.0", "readme", "# Hi <b>", options, { store: s, render: dies, keep });
      expect(tries).toBe(2);
      // A second death: held an hour now, then six.
      at(s, HOLDS[0] + 1);
      await drawn("https://zeron.sh", "a", "1.0.0", "readme", "# Hi <b>", options, { store: s, render: dies, keep });
      expect(tries).toBe(2);
      at(s, HOLDS[1]);
      await drawn("https://zeron.sh", "a", "1.0.0", "readme", "# Hi <b>", options, { store: s, render: dies, keep });
      expect(tries).toBe(3);
      at(s, HOLDS[1] + 1);
      await drawn("https://zeron.sh", "a", "1.0.0", "readme", "# Hi <b>", options, { store: s, render: dies, keep });
      expect(tries).toBe(3);
      at(s, HOLDS[2]);
      await drawn("https://zeron.sh", "a", "1.0.0", "readme", "# Hi <b>", options, { store: s, render: dies, keep });
      expect(tries).toBe(4);
    }
  });

  test("a renderer that never returns (the kill the local runtime can't do): the next request shows the text, without trying", async () => {
    const store = memory();
    let tries = 0;
    const never = (): Promise<Rendered> => (tries++, new Promise<Rendered>(() => {}));
    void drawn("https://zeron.sh", "a", "1.0.0", "notes", "Some *notes*.", options, { store, render: never, keep: true });
    // (The try is recorded, and awaited, before the renderer is called.)
    await new Promise((resolve) => setTimeout(resolve, 10));
    expect(tries).toBe(1);
    const next = await drawn("https://zeron.sh", "a", "1.0.0", "notes", "Some *notes*.", options, { store, render: never, keep: true });
    expect(next).toEqual({ html: expect.stringContaining("Shown as plain text: it couldn't be laid out."), cut: "attempt" });
    expect(tries).toBe(1);
    // After the hold, one more try, and this time it finishes: drawn, kept, and the tries forgotten.
    at(store, HOLDS[0]);
    const drawnNow = await drawn("https://zeron.sh", "a", "1.0.0", "notes", "Some *notes*.", options, { store, keep: true });
    expect(drawnNow).toEqual({ html: expect.stringContaining("<em>notes</em>"), cut: null });
    expect(store.size()).toBe(1);
  });

  test("the renderer's own failure is held like a death, never kept as a fact", async () => {
    const store = memory();
    const failing = (md: string): Rendered => ({ html: `<p class="as-text">x</p>${md}`, cut: "error" });
    const first = await drawn("https://zeron.sh", "a", "1.0.0", "readme", "x", options, { store, render: failing, keep: true });
    expect(first.cut).toBe("error");
    expect((await drawn("https://zeron.sh", "a", "1.0.0", "readme", "x", options, { store, render: failing, keep: true })).cut).toBe("attempt");
    at(store, HOLDS[0]);
    expect((await drawn("https://zeron.sh", "a", "1.0.0", "readme", "x", options, { store, keep: true })).cut).toBe(null);
  });

  test("where nothing is kept (a version in review), a finished try takes its record away", async () => {
    const store = memory();
    let renders = 0;
    const render = (md: string, o: RenderOptions): Rendered => (renders++, renderWithin(md, o));
    await drawn("https://zeron.sh", "a", "1.0.0", "readme", "# Hi", options, { store, render, keep: false });
    await drawn("https://zeron.sh", "a", "1.0.0", "readme", "# Hi", options, { store, render, keep: false });
    expect(renders).toBe(2);
    expect(store.size()).toBe(0);
  });
});

describe("a try is the request's to finish, not the visitor's", () => {
  const options = { file: (p: string) => `/plugins/a/files/1.0.0/${p}`, demote: true };

  test("a visitor who goes away at once can't hold an honest README: the try finishes under waitUntil, and the next visit is drawn", async () => {
    const store = memory();
    let renders = 0;
    const render = (md: string, o: RenderOptions): Rendered => (renders++, renderWithin(md, o));
    const pending: Promise<unknown>[] = [];
    const waitUntil = (p: Promise<unknown>) => void pending.push(p);
    // (The visitor's request is dropped: no one awaits what it returns. The platform still finishes what waitUntil holds.)
    void drawn("https://zeron.sh", "a", "1.0.0", "readme", "# Hi", options, { store, render, keep: true, waitUntil });
    // (Held from the moment the try begins: before it, the key's hash and the look for a kept one record nothing.)
    await new Promise((resolve) => setTimeout(resolve, 10));
    expect(pending).toHaveLength(1);
    await Promise.all(pending);
    const next = await drawn("https://zeron.sh", "a", "1.0.0", "readme", "# Hi", options, { store, render, keep: true });
    expect(next.cut).toBe(null);
    expect(next.html).toContain("Hi");
    expect(renders).toBe(1);
    // (Three dropped visits in a row still leave it drawn, and no count of deaths behind.)
    for (let i = 0; i < 3; i++) void drawn("https://zeron.sh", "a", "1.0.0", "readme", "# Other", options, { store, render, keep: true, waitUntil });
    await new Promise((resolve) => setTimeout(resolve, 10));
    await Promise.all(pending);
    expect((await drawn("https://zeron.sh", "a", "1.0.0", "readme", "# Other", options, { store, render, keep: true })).cut).toBe(null);
    expect(store.size()).toBe(2);
  });

  test("a result that can't be written takes its record and count away: the next visit draws again, and isn't held", async () => {
    const store = memory();
    const put = store.put;
    // (The fault: the cache refuses the result's write, after a rendering that succeeded.)
    store.put = async (r, response) => {
      if (response.headers.get("x-drawn") === "done") throw new Error("the cache refused it");
      return put(r, response);
    };
    let renders = 0;
    const render = (md: string, o: RenderOptions): Rendered => (renders++, renderWithin(md, o));
    expect((await drawn("https://zeron.sh", "a", "1.0.0", "readme", "# Hi", options, { store, render, keep: true })).cut).toBe(null);
    expect((await drawn("https://zeron.sh", "a", "1.0.0", "readme", "# Hi", options, { store, render, keep: true })).cut).toBe(null);
    expect(renders).toBe(2);
    // (Nothing left: no record, no count. A death now would be held five minutes, its first.)
    expect(store.size()).toBe(0);
    const dies = (): Rendered => {
      throw new Error("killed");
    };
    await drawn("https://zeron.sh", "a", "1.0.0", "readme", "# Hi", options, { store, render: dies, keep: true });
    store.clock.now += HOLDS[0];
    expect((await drawn("https://zeron.sh", "a", "1.0.0", "readme", "# Hi", options, { store, render, keep: true })).cut).toBe(null);
  });

  test("the review page's tries are its own: they never stand in for, or take away, the public page's", async () => {
    const store = memory();
    const md = "# Listed, and opened in review";
    const shown = await drawn("https://zeron.sh", "a", "1.0.0", "readme", md, options, { store, keep: true });
    expect(shown.cut).toBe(null);
    // A review try that never finishes: the public page still finds its rendering.
    void drawn("https://zeron.sh", "a", "1.0.0", "readme", md, options, { store, keep: false, render: () => new Promise<Rendered>(() => {}) });
    await new Promise((resolve) => setTimeout(resolve, 10));
    expect((await drawn("https://zeron.sh", "a", "1.0.0", "readme", md, options, { store, keep: true, render: () => { throw new Error("not drawn again"); } })).cut).toBe(null);
    // A review try that finishes takes away only its own record.
    store.clock.now += HOLDS[0];
    await drawn("https://zeron.sh", "a", "1.0.0", "readme", md, options, { store, keep: false });
    expect((await drawn("https://zeron.sh", "a", "1.0.0", "readme", md, options, { store, keep: true, render: () => { throw new Error("not drawn again"); } })).cut).toBe(null);
  });

  test("a page showing a held or failed document isn't kept by shared caches; one drawn, or too long, is", () => {
    const as = (cut: Rendered["cut"] | "attempt") => ({ html: "", cut });
    expect(keepable([as(null), as("size"), as("keep")])).toBe(true);
    expect(keepable([as(null), as("attempt")])).toBe(false);
    expect(keepable([as("error")])).toBe(false);
  });
});
