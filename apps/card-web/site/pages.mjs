// The card's pages as strings: the share page at /@username (X's Player Card
// and Open Graph tags, the card itself, a line of facts), the player (the
// iframe X shows inside a post: the card alone), the embed test page and the
// mock index. No dependencies: the edge worker imports it as it is
// (edge/src/card-routes.ts) and the static build (tools/build.mjs) too.
//
// A page shows only the public card (edge/src/user-directory.ts's
// `PublicCard`: username, firstName, userNumber, joinedOn, badges, scene,
// holo, sky) and embeds exactly that as its `#card` data block, nothing
// else; the art is resolved on the client from the scenes manifest by the
// public `scene`. Member-written text (a first name, a badge's name) lands
// in HTML on the site's origin, so every string goes through `escapeHtml`,
// and the data block escapes `<` so nothing can close it early. The still
// picture is the fallback for a card that can't draw: its `<img>` names it
// in `data-src` and card-page.js loads it only then, so a page view never
// fetches it, and the browser drawing the picture (card-image.ts loads the
// player) never asks for the picture it is making.

export const escapeHtml = (s) =>
  String(s).replace(/[&<>"']/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" })[c]);

/** The public card's keys, in the order the edge serves them; a page embeds
 * these and only these. */
export const PUBLIC_KEYS = ["username", "firstName", "userNumber", "joinedOn", "badges", "scene", "holo", "sky"];

/** Exactly the public card: the known keys in their order, badges reduced
 * to id, name and icon. Anything else a caller passes is dropped. */
export function publicCard(card) {
  return {
    username: String(card.username),
    firstName: card.firstName == null ? null : String(card.firstName),
    userNumber: Number(card.userNumber),
    joinedOn: String(card.joinedOn),
    badges: (card.badges || []).map((b) => ({ id: String(b.id), name: String(b.name), icon: b.icon == null ? null : String(b.icon) })),
    scene: Number(card.scene),
    holo: String(card.holo),
    sky: Number(card.sky)
  };
}

const dataBlock = (card) => `<script type="application/json" id="card">${JSON.stringify(publicCard(card)).replaceAll("<", "\\u003c")}</script>`;

/** What a card is called on its pages: the first name, or the handle. */
export const nameOf = (card) => (card.firstName ? String(card.firstName) : `@${card.username}`);

// How big the player is on X: square, as the reference; the card scales to
// fit it with room for its shadow.
export const PLAYER = { width: 480, height: 480 };
// The still picture X shows where it won't load the player: the same shape
// as the player, at 2.5x.
export const IMAGE = { width: 1200, height: 1200 };

/** The page's URLs. `site`: the public origin (https://zeron.sh); `base`:
 * where card.js, the fonts and the art live (`<site>/card` by default). */
export function urlsFor(card, site, base = `${site}/card`) {
  const u = encodeURIComponent(String(card.username || ""));
  return {
    page: `${site}/c/${u}`,
    player: `${site}/c/${u}/player`,
    image: `${site}/c/${u}/card.png`,
    json: `${site}/cards/${u}`,
    script: `${base}/card.js`,
    page_script: `${base}/card-page.js`,
    style: `${base}/card.css`,
    manifest: `${base}/art/manifest.json`
  };
}

const head = (title, description, urls, extra = "") => `<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8" />
<meta name="viewport" content="width=device-width, initial-scale=1, viewport-fit=cover" />
<title>${escapeHtml(title)}</title>
<meta name="description" content="${escapeHtml(description)}" />
<link rel="stylesheet" href="${urls.style}" />
${extra}</head>`;

/** The share page: what a post links to. Its meta tags make X show the
 * player; its body shows the same card for anyone who opens the link.
 * `options.site`: the X handle for `twitter:site`. */
export function renderShare(card, urls, options = {}) {
  const name = nameOf(card);
  const title = `${name} — Zeron member #${Number(card.userNumber)}`;
  const description = `${name}'s Zeron member card: #${Number(card.userNumber)}, joined ${card.joinedOn}.`;
  const alt = `${name}'s Zeron member card`;
  const meta = `
<meta name="twitter:card" content="player" />
${options.site ? `<meta name="twitter:site" content="${escapeHtml(options.site)}" />` : ""}
<meta name="twitter:title" content="${escapeHtml(title)}" />
<meta name="twitter:description" content="${escapeHtml(description)}" />
<meta name="twitter:image" content="${urls.image}" />
<meta name="twitter:image:alt" content="${escapeHtml(alt)}" />
<meta name="twitter:player" content="${urls.player}" />
<meta name="twitter:player:width" content="${PLAYER.width}" />
<meta name="twitter:player:height" content="${PLAYER.height}" />
<meta property="og:type" content="website" />
<meta property="og:url" content="${urls.page}" />
<meta property="og:title" content="${escapeHtml(title)}" />
<meta property="og:description" content="${escapeHtml(description)}" />
<meta property="og:image" content="${urls.image}" />
<meta property="og:image:width" content="${IMAGE.width}" />
<meta property="og:image:height" content="${IMAGE.height}" />
<meta property="og:image:alt" content="${escapeHtml(alt)}" />
<link rel="canonical" href="${urls.page}" />
<style>
  /* The card alone, filling the window, nothing to scroll to. */
  html, body { height: 100%; overflow: hidden; overscroll-behavior: none; }
  main { height: 100vh; height: 100dvh; }
  .stage { height: 100%; }
</style>`;
  return `${head(title, description, urls, meta)}
<body>
<main>
  <div class="stage" id="stage" data-mode="share"><img class="fallback" alt="${escapeHtml(alt)}" data-src="${urls.image}" /></div>
</main>
${dataBlock(card)}
<script type="module" src="${urls.page_script}"></script>
</body>
</html>
`;
}

/** The player: the card alone, filling the iframe. Its query parameters
 * (`?theme`, `?scale`, `?origin`, `?pose`, `?lit`, `?drift`, `?tilt`,
 * `?arrive`, `?still`, `?caption`, `?dpr`, `?frame`) drive captures and
 * tests; see card-page.js. */
export function renderPlayer(card, urls) {
  const name = nameOf(card);
  const alt = `${name}'s Zeron member card`;
  const style = `<style>
  html, body { height: 100%; overflow: hidden; }
  .stage { position: absolute; inset: 0; }
</style>`;
  return `${head(`${name} — Zeron`, alt, urls, style)}
<body>
<div class="stage" id="stage" data-mode="player"><img class="fallback" alt="${escapeHtml(alt)}" data-src="${urls.image}" /></div>
${dataBlock(card)}
<script type="module" src="${urls.page_script}"></script>
</body>
</html>
`;
}

/** A page that stands in for a post on X: the player in an iframe at the
 * post's size, in a column as wide as a timeline's, with a narrower phone
 * frame too. `?u=` picks the member, `?w=` the column's width. */
export function renderEmbedTest(members, urls) {
  const options = members.map((m) => `<option value="${escapeHtml(m.username)}">@${escapeHtml(m.username)}</option>`).join("");
  return `<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8" />
<meta name="viewport" content="width=device-width, initial-scale=1" />
<title>Embed test</title>
<link rel="stylesheet" href="${urls.style}" />
<style>
  body { padding: 24px 16px; display: grid; gap: 24px; justify-items: center; }
  .post { width: var(--w, 480px); max-width: 100%; border: 1px solid var(--faint); border-radius: 16px; padding: 12px 16px; display: grid; gap: 12px; }
  .post header { display: flex; gap: 8px; align-items: center; font-size: 15px; }
  .post header i { width: 40px; height: 40px; border-radius: 50%; background: var(--faint); display: inline-block; }
  .post header span { color: var(--muted); }
  .post .frame { border-radius: 16px; overflow: hidden; border: 1px solid var(--faint); aspect-ratio: ${PLAYER.width} / ${PLAYER.height}; }
  .post iframe { display: block; width: 100%; height: 100%; border: 0; }
  .controls { display: flex; gap: 8px; flex-wrap: wrap; align-items: center; font-size: 13px; color: var(--muted); }
  .controls select, .controls input { font: inherit; }
  .report { font: 12px/1.5 "Geist Mono", ui-monospace, monospace; color: var(--muted); white-space: pre-wrap; max-width: 640px; }
</style>
</head>
<body>
<div class="controls">
  <label>member <select id="member">${options}</select></label>
  <label>width <input id="width" type="number" value="480" min="200" max="900" step="1" /></label>
  <label><input id="mobile" type="checkbox" /> phone column (375)</label>
</div>
<article class="post" id="post">
  <header><i></i><div><b>Zeron</b> <span>@zeron · 1m</span></div></header>
  <p id="text">My Zeron member card.</p>
  <div class="frame"><iframe id="player" title="Member card" allow="autoplay" sandbox="allow-scripts allow-same-origin"></iframe></div>
</article>
<div class="report" id="report"></div>
<script type="module">
const q = new URLSearchParams(location.search);
const post = document.getElementById("post"), frame = document.getElementById("player");
const member = document.getElementById("member"), width = document.getElementById("width"), mobile = document.getElementById("mobile");
if (q.get("u")) member.value = q.get("u");
if (q.get("w")) width.value = q.get("w");
function apply() {
  const w = mobile.checked ? 375 - 32 : Number(width.value);
  post.style.setProperty("--w", w + "px");
  frame.src = "/@" + encodeURIComponent(member.value) + "/player";
  const fw = frame.getBoundingClientRect();
  document.getElementById("report").textContent = "iframe " + Math.round(fw.width) + "x" + Math.round(fw.height) + " CSS px, dpr " + devicePixelRatio + ", src " + frame.src;
}
member.onchange = width.oninput = mobile.onchange = apply;
apply();
</script>
</body>
</html>
`;
}

export function renderIndex(members, urls) {
  const rows = members.map((m) => {
    const u = encodeURIComponent(m.username);
    return `<li><a href="/@${u}"><b>${escapeHtml(nameOf(m))}</b> @${escapeHtml(m.username)} · #${Number(m.userNumber)}</a> <a class="small" href="/@${u}/player">player</a> <a class="small" href="/cards/${u}">json</a> <a class="small" href="/@${u}/card.png">image</a></li>`;
  }).join("\n");
  return `<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8" />
<meta name="viewport" content="width=device-width, initial-scale=1" />
<title>Member cards</title>
<link rel="stylesheet" href="${urls.style}" />
<style>
  main { max-width: 640px; margin: 0 auto; padding: 48px 16px; }
  ul { list-style: none; padding: 0; display: grid; gap: 8px; }
  a { color: var(--ink); text-decoration: none; }
  a.small { color: var(--muted); font-size: 13px; margin-left: 8px; }
</style>
</head>
<body>
<main>
<h1>Member cards</h1>
<p>Fictional members, each at <code>/@username</code> as the edge serves a real one. <a href="/embed-test.html">Embed test</a> shows the player inside a post.</p>
<ul>
${rows}
</ul>
</main>
</body>
</html>
`;
}
