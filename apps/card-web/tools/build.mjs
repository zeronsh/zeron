// Builds the static site into dist/: the card's script, style and fonts, the
// scenes' art and manifest (from apps/zgx), and for each mock member the
// share page (/@u), the player (/@u/player), the public card (/cards/u) and
// a place for its picture (/@u/card.png, made by images.mjs). Plus the index,
// the embed test page and a _headers file (Cloudflare's format) with the
// headers the player needs. `node tools/build.mjs [--site https://zeron.sh]`.
import { cp, mkdir, readFile, rm, writeFile } from "node:fs/promises";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { artFor } from "../site/card.js";
import { publicCard, renderEmbedTest, renderIndex, renderPlayer, renderShare, urlsFor } from "../site/pages.mjs";

/** The static files a card page needs, into `into` (served at /card): the
 * script, the page script, the style, the fonts and the art with its
 * manifest. The edge's deploy copies the same into its public/ folder. */
export async function copyAssets(into) {
  await mkdir(into, { recursive: true });
  for (const f of ["card.js", "card-page.js", "card.css"]) await cp(join(root, "site", f), join(into, f));
  await cp(join(root, "site", "fonts"), join(into, "fonts"), { recursive: true });
  await cp(join(root, "art"), join(into, "art"), { recursive: true });
}

const here = dirname(fileURLToPath(import.meta.url));
const root = join(here, "..");
export const dist = join(root, "dist");

const arg = (name, fallback) => {
  const i = process.argv.indexOf(name);
  return i >= 0 ? process.argv[i + 1] : fallback;
};

// The player's CSP: no scripts but its own, framed only by X and itself.
const FRAME_ANCESTORS = "frame-ancestors 'self' https://x.com https://*.x.com https://twitter.com https://*.twitter.com";
export const PLAYER_CSP = `default-src 'none'; script-src 'self'; style-src 'self' 'unsafe-inline'; img-src 'self' data:; font-src 'self'; connect-src 'self'; ${FRAME_ANCESTORS}`;
export const PAGE_CSP = `default-src 'none'; script-src 'self'; style-src 'self' 'unsafe-inline'; img-src 'self' data:; font-src 'self'; connect-src 'self'; frame-ancestors 'self'`;

export async function build({ site = "" } = {}) {
  const members = JSON.parse(await readFile(join(root, "mock", "members.json"), "utf8")).members;
  const manifest = JSON.parse(await readFile(join(root, "art", "manifest.json"), "utf8"));
  await rm(dist, { recursive: true, force: true });
  await mkdir(join(dist, "cards"), { recursive: true });
  await copyAssets(join(dist, "card"));
  for (const m of members) {
    const card = publicCard(m);
    const urls = urlsFor(card, site);
    const dir = join(dist, `@${m.username}`);
    await mkdir(join(dir, "player"), { recursive: true });
    await writeFile(join(dir, "index.html"), renderShare(card, urls, { site: "@zeron" }));
    await writeFile(join(dir, "player", "index.html"), renderPlayer(card, urls));
    await writeFile(join(dist, "cards", `${m.username}.json`), JSON.stringify(card));
  }
  // The gallery's cards as comparison fixtures (tools/fixtures/gallery.json):
  // players only, under /_gallery, with their taglines and badge styles.
  // The gallery's cards as comparison fixtures (tools/fixtures/gallery.json):
  // players only, under /_gallery, with their taglines, badge styles and art
  // inline (fields the public card doesn't carry, so these pages render the
  // data block themselves).
  const fixtures = JSON.parse(await readFile(join(here, "fixtures", "gallery.json"), "utf8")).members;
  for (const m of fixtures) {
    const card = { ...publicCard({ ...m, firstName: m.displayName ?? m.firstName }), tagline: m.tagline, badges: m.badges, art: artFor(manifest, m.scene, "/card/art") };
    const urls = urlsFor(card, site);
    const dir = join(dist, "_gallery", m.username, "player");
    await mkdir(dir, { recursive: true });
    const html = renderPlayer(card, urls).replace(/<script type="application\/json" id="card">.*?<\/script>/s, `<script type="application/json" id="card">${JSON.stringify(card).replaceAll("<", "\\u003c")}</script>`);
    await writeFile(join(dir, "index.html"), html);
  }
  const urls = urlsFor({ username: "" }, site);
  await writeFile(join(dist, "index.html"), renderIndex(members, urls));
  await writeFile(join(dist, "embed-test.html"), renderEmbedTest(members, urls));
  await writeFile(join(dist, "_headers"), `/@*/player
  Content-Security-Policy: ${PLAYER_CSP}
  Cache-Control: public, max-age=60
  X-Content-Type-Options: nosniff
/@*
  Content-Security-Policy: ${PAGE_CSP}
  Cache-Control: public, max-age=60
  X-Content-Type-Options: nosniff
/cards/*
  Access-Control-Allow-Origin: *
  Cache-Control: public, max-age=3600
/card/card.js
  Cache-Control: public, max-age=300
/card/card-page.js
  Cache-Control: public, max-age=300
/card/fonts/*
  Cache-Control: public, max-age=31536000, immutable
/card/art/*
  Cache-Control: public, max-age=86400
`);
  return members;
}

if (process.argv[1] && fileURLToPath(import.meta.url) === process.argv[1]) {
  const members = await build({ site: arg("--site", "") });
  console.log(`built ${members.length} members into ${dist}`);
}
