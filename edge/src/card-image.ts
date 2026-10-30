/**
 * A card's still picture (`/@:username/card.png`): what X shows in place of
 * the player, and the share page's fallback. It is the player page itself,
 * drawn by a headless browser (Browser Run) at the post's size, 2.5 device
 * pixels a CSS pixel, the card at rest; stripped of every PNG chunk but the
 * picture; kept in R2 under a key that hashes the public card, so a changed
 * card (a new badge, a renamed member) is drawn again and an unchanged one
 * is never drawn twice. Without a browser binding (tests, a deployment
 * without Browser Run) a picture not yet in R2 answers 404, cached briefly.
 */
import { IMAGE, PLAYER, urlsFor } from "../../apps/card-web/site/pages.mjs";
import type { Env } from "./env";
import { stripPng } from "./png";
import type { PublicCard } from "./user-directory";

/** Bump when the drawing changes, so every picture is drawn again. */
export const RENDER_VERSION = 1;
/** Seconds a picture may be served from a browser or X's cache. */
const IMAGE_TTL_SECONDS = 3600;
/** Seconds to wait before asking again for a picture that couldn't be drawn. */
const MISSING_IMAGE_TTL_SECONDS = 60;

const hex = (buffer: ArrayBuffer): string => [...new Uint8Array(buffer)].map((b) => b.toString(16).padStart(2, "0")).join("");

/** Where the picture of exactly this card lives in R2. */
export const imageKey = async (card: PublicCard): Promise<string> => {
  const digest = await crypto.subtle.digest("SHA-256", new TextEncoder().encode(`${JSON.stringify(card)}#${RENDER_VERSION}`));
  return `cards/${card.username}/${hex(digest).slice(0, 24)}.png`;
};

const png = (bytes: ArrayBuffer | Uint8Array, etag?: string): Response =>
  new Response(bytes as BodyInit, {
    status: 200,
    headers: {
      "content-type": "image/png",
      "cache-control": `public, max-age=${IMAGE_TTL_SECONDS}`,
      "x-content-type-options": "nosniff",
      ...(etag ? { etag } : {})
    }
  });

const notYet = (): Response =>
  new Response(JSON.stringify({ error: "not_found", message: "No picture of this card yet." }), {
    status: 404,
    headers: { "content-type": "application/json", "cache-control": `public, max-age=${MISSING_IMAGE_TTL_SECONDS}` }
  });

/** The picture for `card`, from R2 or drawn now; `site` is the origin the
 * player page lives on (the request's own). */
export const cardImage = async (card: PublicCard, env: Env, ctx: ExecutionContext, site: string): Promise<Response> => {
  const key = await imageKey(card);
  const bucket = env.CARD_IMAGES;
  if (bucket) {
    const kept = await bucket.get(key);
    if (kept) return png(await kept.arrayBuffer(), kept.httpEtag);
  }
  if (!env.BROWSER) return notYet();
  let bytes: Uint8Array;
  try {
    bytes = await draw(env.BROWSER, urlsFor(card, site).player);
  } catch (e) {
    console.error("cards: drawing a picture failed", String(e));
    return notYet();
  }
  if (bucket) ctx.waitUntil(bucket.put(key, bytes, { httpMetadata: { contentType: "image/png" } }));
  return png(bytes);
};

/** The player page screenshotted by Browser Run: the card at rest, no
 * arrival, in the dark window (the picture stands alone in a post). */
const draw = async (binding: Fetcher, playerUrl: string): Promise<Uint8Array> => {
  const { default: puppeteer } = await import("@cloudflare/puppeteer");
  const browser = await puppeteer.launch(binding as never);
  try {
    const page = await browser.newPage();
    await page.setViewport({ width: PLAYER.width, height: PLAYER.height, deviceScaleFactor: IMAGE.width / PLAYER.width });
    await page.goto(`${playerUrl}?arrive=0&still=1&theme=dark`, { waitUntil: "load", timeout: 20_000 });
    // The page's own promise of its first drawn frame (card-page.js), as an
    // expression: the Worker has no DOM types, and puppeteer awaits it.
    const drawn = (await page.evaluate("window.__cardReady")) as boolean;
    if (!drawn) throw new Error("the card did not draw (no WebGL?)");
    await page.evaluate("new Promise((r) => requestAnimationFrame(() => requestAnimationFrame(r)))");
    const shot = await page.screenshot({ type: "png" });
    return stripPng(new Uint8Array(shot as Uint8Array));
  } finally {
    await browser.close();
  }
};
