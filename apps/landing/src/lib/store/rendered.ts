// A version's README and notes never change (a version belongs to what was
// submitted for good), so each is rendered once and kept in the Worker's
// cache, by plugin, version and renderer: a page that shows it again reads
// it back instead of parsing it again. Only listed versions' (public) are
// kept, under a host no visitor can ask for, so a kept rendering is never
// an answer to anyone's request.
import { renderMarkdown, RENDERER, type RenderOptions } from "./markdown.ts";

const DAY = 86_400;

/** The HTML of `markdown`, which is `what` of `id` at `version`. */
export async function renderedOnce(id: string, version: string, what: "readme" | "notes", markdown: string, options: RenderOptions): Promise<string> {
  const store = typeof caches === "undefined" ? null : (caches as unknown as { default: Cache }).default;
  const key = new Request(`https://rendered.zeron.invalid/${RENDERER}/${encodeURIComponent(id)}/${encodeURIComponent(version)}/${what}${options.demote ? "" : "/flat"}`);
  if (store) {
    const kept = await store.match(key).catch(() => undefined);
    if (kept) return kept.text();
  }
  const html = renderMarkdown(markdown, options);
  await store?.put(key, new Response(html, { headers: { "content-type": "text/html; charset=utf-8", "cache-control": `public, max-age=${30 * DAY}` } })).catch(() => {});
  return html;
}
