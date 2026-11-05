// Mounts the card on a page: reads the public card from the page's `#card`
// data block, resolves its art from the scenes manifest next to this script
// (by the public `scene`), and for the player reads the query's rig
// (`?theme`, `?scale`, `?origin`, `?pose`, `?lit`, `?strength`, `?drift`,
// `?tilt`, `?arrive`, `?still`, `?caption`, `?dpr`, `?frame`) for captures
// and tests. An external script, so the pages' Content-Security-Policy can
// refuse inline ones. Nothing here is logged.
import { artFor, mountCard } from "./card.js";

const stage = document.getElementById("stage");
const mode = stage.dataset.mode || "player";
const card = JSON.parse(document.getElementById("card").textContent);
const q = new URLSearchParams(location.search);
const pair = (v) => (v ? v.split(",").map(Number) : null);
const artBase = new URL("art", import.meta.url).href;

async function withArt(c) {
  if (c.art && c.art.layers) return c;
  const manifest = await (await fetch(`${artBase}/manifest.json`)).json();
  return { ...c, art: artFor(manifest, c.scene, artBase) };
}

function playerOptions() {
  if (q.get("theme")) document.documentElement.dataset.theme = q.get("theme");
  if (q.get("frame")) {
    const [w, h] = q.get("frame").split("x").map(Number);
    stage.style.width = `${w}px`; stage.style.height = `${h}px`; stage.style.inset = "auto";
  }
  const rig = q.has("pose") || q.has("lit") || q.has("drift") ? {
    pose: q.get("pose") && q.get("pose") !== "rest" ? pair(q.get("pose")) : null,
    lit: q.get("lit") === "1",
    strength: q.has("strength") ? Number(q.get("strength")) : undefined,
    drift: q.has("drift") ? (q.get("drift").includes(",") ? pair(q.get("drift")) : Number(q.get("drift"))) : undefined,
    tilt: q.get("tilt") !== "0"
  } : null;
  const origin = pair(q.get("origin"));
  return {
    theme: q.get("theme") || "auto",
    caption: q.get("caption") === "1",
    arrive: q.get("arrive") !== "0" && !rig,
    still: q.has("still") ? q.get("still") === "1" : "auto",
    scale: q.has("scale") ? Number(q.get("scale")) : "fit",
    origin: origin ? { x: origin[0], y: origin[1] } : null,
    dpr: q.has("dpr") ? Number(q.get("dpr")) : null,
    rig
  };
}

stage.addEventListener("cardfailed", () => {
  stage.classList.add("failed");
  // The still picture, only now: a page that draws never fetches it.
  const fallback = stage.querySelector("img.fallback");
  if (fallback && fallback.dataset.src && !fallback.src) fallback.src = fallback.dataset.src;
});
window.__cardReady = (async () => {
  const full = await withArt(card);
  const options = mode === "player" ? playerOptions() : { arrive: true, caption: false };
  const controller = mountCard(stage, full, options);
  window.__card = controller;
  await controller.ready;
  await new Promise((r) => requestAnimationFrame(() => requestAnimationFrame(r)));
  return true;
})().catch(() => false);
