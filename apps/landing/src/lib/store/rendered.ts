// A plugin's README and a version's notes, drawn: once, kept, and never
// tried again while a try is under way or just died.
//
// Kept: a listed version's README and notes don't change, so each is
// rendered once and kept in the Worker's cache: a page that shows it again
// reads it back instead of parsing it again. The key is the hash of what
// the rendering is made from (the pipeline, the site's origin, where its
// pictures come from, the page's level and title, and the text, as a JSON
// array), so a rendering can only ever stand for the very text it was made
// from. It is kept under a host no visitor can ask for, so a kept
// rendering is never an answer to anyone's request.
//
// Tried once: the renderer is linear by construction (md/), and the site's
// CPU limit is a backstop; neither may be all that stands between a
// document that still beats them and a page that fails on every visit, or
// a hostile submission that hides itself from its reviewer. So a try is
// recorded before it is made: "attempting", under the same key, written and
// awaited first; then the rendering; then the result over it (or, where
// nothing is kept, the record taken away). A request that finds a try
// recorded and no result does not render: it shows the document as its
// text, and says so (on the staff side, loudly: drawing.ts).
//
// A try is held, not kept: a kill can be a passing thing (an isolate
// restarted, a deploy), and a failure must not stand as a fact about the
// document. The first is held HOLDS[0] (five minutes: soon enough to try
// again, long enough that a document that kills each time costs one death
// in five minutes a place); if the next try dies too, an hour; after that,
// six hours. What counts the tries lives a day, and goes when a try
// finishes. Two first visits that both miss the record each try once: that
// is accepted.
//
// A try is the request's to finish, not the visitor's: the whole of it
// (the record, the rendering, then the result kept or the record taken
// away) runs under the Worker's waitUntil, so a visitor who goes away at
// once can't stop it between its record and its outcome, and leave an
// honest document held as if it had died. A cancelled request then always
// ends in an outcome; only a try the platform kills (past its CPU or its
// memory, which ends waitUntil's work with it) leaves its record, and only
// that is counted as a death. A result that can't be written (the cache
// failing) takes the record and the count away rather than leave them.
//
// The review page's tries are its own (`review-` before the document's
// name in the key): it keeps nothing, and its records must never stand in
// for, or take away, the public page's.

import { PIPELINE, render as renderWithin, type Cut, type Rendered, type RenderOptions } from "./markdown.ts";
import { asText } from "./md/html.ts";

const MINUTE = 60;
const HOUR = 60 * MINUTE;
const DAY = 24 * HOUR;

/** How long a try is held, by how many tries before it never finished (see above). */
export const HOLDS = [5 * MINUTE, HOUR, 6 * HOUR] as const;

/** Why a document is shown as its text: the renderer's own cut, or a try recorded and not finished. */
export type Why = Cut | "attempt";

export interface Drawn {
  html: string;
  cut: Why | null;
}

async function sha256(text: string): Promise<string> {
  const digest = await crypto.subtle.digest("SHA-256", new TextEncoder().encode(text));
  return [...new Uint8Array(digest)].map((b) => b.toString(16).padStart(2, "0")).join("");
}

/** Where renderings and tries are kept: the Worker's cache (a test hands in its own). */
export interface Keeper {
  match(request: Request): Promise<Response | undefined>;
  put(request: Request, response: Response): Promise<void>;
  delete(request: Request): Promise<boolean>;
}

const workerCache = (): Keeper | null => (typeof caches === "undefined" ? null : (caches as unknown as { default: Keeper }).default);

const kept = (body: string, state: "done" | "attempting", seconds: number, cut: Why | null = null) =>
  new Response(body, {
    headers: { "content-type": "text/html; charset=utf-8", "cache-control": `public, max-age=${seconds}`, "x-drawn": state, ...(cut ? { "x-cut": cut } : {}) },
  });

export interface Drawing {
  /** Keep the result (a listed version's, the public page): or only record the try (a version in review is no one's to keep). */
  keep: boolean;
  /** The Worker's waitUntil: the try is finished under it, whether or not the visitor stays. */
  waitUntil?: (promise: Promise<unknown>) => void;
  store?: Keeper | null;
  pipeline?: string;
  render?: (markdown: string, options: RenderOptions) => Rendered | Promise<Rendered>;
}

/**
 * `markdown`, which is `what` of `id` at `version`, on the site at
 * `origin`, drawn: kept when it was (`keep`), shown as its text when a try
 * is held, and otherwise tried, the try recorded first.
 */
export async function drawn(
  origin: string,
  id: string,
  version: string,
  what: "readme" | "notes",
  markdown: string,
  options: RenderOptions,
  drawing: Drawing,
): Promise<Drawn> {
  const store = drawing.store === undefined ? workerCache() : drawing.store;
  const render = drawing.render ?? renderWithin;
  const pictures = options.file("x");
  // (Its fields as a JSON array: unambiguous by construction, whatever a field holds.)
  const hash = await sha256(JSON.stringify([drawing.pipeline ?? PIPELINE, origin, pictures, options.demote ? 1 : 0, options.title ?? "", markdown]));
  const name = drawing.keep ? what : `review-${what}`;
  const key = new Request(`https://rendered.zeron.invalid/${encodeURIComponent(id)}/${encodeURIComponent(version)}/${name}/${hash}`);
  const tries = new Request(`${key.url}/tries`);
  const failed = (): Drawn => ({ html: asText(markdown, "failed"), cut: "attempt" });

  if (store) {
    const found = await store.match(key).catch(() => undefined);
    if (found?.headers.get("x-drawn") === "done") return { html: await found.text(), cut: (found.headers.get("x-cut") as Why | null) ?? null };
    if (found?.headers.get("x-drawn") === "attempting") return failed();
  }

  const attempt = (async (): Promise<Drawn> => {
    if (store) {
      // The try, recorded before it is made (and awaited: a try that dies leaves it).
      const before = Number((await (await store.match(tries).catch(() => undefined))?.text()) ?? 0) || 0;
      await store.put(key, kept("", "attempting", HOLDS[Math.min(before, HOLDS.length - 1)])).catch(() => {});
      await store.put(tries, kept(String(before + 1), "attempting", DAY)).catch(() => {});
    }
    let result: Rendered;
    try {
      result = await render(markdown, options);
    } catch {
      // (A fault, caught: as a try that died. Its record stays, and holds it.)
      return failed();
    }
    // The renderer's own failure is no fact about the document either: held, not kept.
    if (result.cut === "error") return result;
    if (store) {
      let wrote = true;
      if (drawing.keep) wrote = await store.put(key, kept(result.html, "done", 30 * DAY, result.cut)).then(() => true, () => false);
      // (Nothing kept, or the result couldn't be: the record goes, so it holds nothing back.)
      if (!drawing.keep || !wrote) await store.delete(key).catch(() => false);
      await store.delete(tries).catch(() => false);
    }
    return result;
  })();
  // (Finished whether or not the visitor stays: held in the same tick the try begins, before its record is
  // written. A request dropped before this point has recorded nothing, so it leaves nothing behind.)
  drawing.waitUntil?.(attempt.catch(() => undefined));
  return attempt;
}

/**
 * Whether a page that shows these may be kept by shared caches: not when
 * any is held (a try that never finished) or failed, which a later visit
 * may draw.
 */
export const keepable = (shown: Drawn[]): boolean => !shown.some((d) => d.cut === "attempt" || d.cut === "error");

/** The HTML of a listed version's document, drawn and kept (the public page). */
export async function renderedOnce(
  origin: string,
  id: string,
  version: string,
  what: "readme" | "notes",
  markdown: string,
  options: RenderOptions,
  drawing: Omit<Drawing, "keep"> = {},
): Promise<string> {
  return (await drawn(origin, id, version, what, markdown, options, { ...drawing, keep: true })).html;
}
