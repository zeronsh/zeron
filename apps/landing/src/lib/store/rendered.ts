// A version's README and notes never change (a version belongs to what was
// submitted for good), so each is rendered once and kept in the Worker's
// cache, by plugin, version and renderer: a page that shows it again reads
// it back instead of parsing it again.
import { renderMarkdown, RENDERER, type RenderOptions } from "./markdown.ts";

const DAY = 86_400;

/** The HTML of `markdown`, which is `what` of `id` at `version`. */
export async function renderedOnce(id: string, version: string, what: "readme" | "notes", markdown: string, options: RenderOptions): Promise<string> {
  const store = typeof caches === "undefined" ? null : (caches as unknown as { default: Cache }).default;
  // (An address of the site's own that no route answers: only this module reads it.)
  const key = new Request(`https://zeron.sh/_rendered/${RENDERER}/${encodeURIComponent(id)}/${encodeURIComponent(version)}/${what}${options.demote ? "" : "/flat"}`);
  if (store) {
    const kept = await store.match(key).catch(() => undefined);
    if (kept) return kept.text();
  }
  const html = renderMarkdown(markdown, options);
  await store?.put(key, new Response(html, { headers: { "content-type": "text/html; charset=utf-8", "cache-control": `public, max-age=${30 * DAY}` } })).catch(() => {});
  return html;
}
