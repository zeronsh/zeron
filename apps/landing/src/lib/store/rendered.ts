// A listed version's README and notes don't change, so each is rendered
// once and kept in the Worker's cache: a page that shows it again reads it
// back instead of parsing it again. The key is the text's own hash (and the
// renderer, the site's origin and where its pictures come from), so a
// rendering can only ever stand for the very text it was made from: not
// another plugin's that later took the same id and version, not staging's.
// Only listed versions' (public) are kept, under a host no visitor can ask
// for, so a kept rendering is never an answer to anyone's request.
import { renderMarkdown, RENDERER, type RenderOptions } from "./markdown.ts";

const DAY = 86_400;

async function sha256(text: string): Promise<string> {
  const digest = await crypto.subtle.digest("SHA-256", new TextEncoder().encode(text));
  return [...new Uint8Array(digest)].map((b) => b.toString(16).padStart(2, "0")).join("");
}

/** The HTML of `markdown`, which is `what` of `id` at `version`, on the site at `origin`. */
export async function renderedOnce(origin: string, id: string, version: string, what: "readme" | "notes", markdown: string, options: RenderOptions): Promise<string> {
  const store = typeof caches === "undefined" ? null : (caches as unknown as { default: Cache }).default;
  const pictures = options.file("x");
  const hash = await sha256(`${RENDERER}\n${origin}\n${pictures}\n${options.demote ? 1 : 0}\n${markdown}`);
  const key = new Request(`https://rendered.zeron.invalid/${encodeURIComponent(id)}/${encodeURIComponent(version)}/${what}/${hash}`);
  if (store) {
    const kept = await store.match(key).catch(() => undefined);
    if (kept) return kept.text();
  }
  const html = renderMarkdown(markdown, options);
  await store?.put(key, new Response(html, { headers: { "content-type": "text/html; charset=utf-8", "cache-control": `public, max-age=${30 * DAY}` } })).catch(() => {});
  return html;
}
