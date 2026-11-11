// The gallery's cards: cut-outs of the Zeron app (its window laid out at
// 1176×768, sidebar closed, as the hero), each over a scene of the Parallax
// scenes plugin under Zeron's ASCII effect (scene.js). Each card's native
// reference is the app's own fixture window in the same state
// (tools/native, `--card`).

import { mountScene } from "./scene.js";
import { shadowUnder } from "./chrome.js";
import { WINDOW, CARD_RADIUS, PAGE, NODE, drawUnder, drawOver, drawPopups, drawReply, iconsReady } from "./chrome.js";

// The Parallax scenes plugin's scenes (scenes.luau), their layers back to front.
const SCENES = {
  wheat: ["back", "far", "near"],
  icebergs: ["back", "far", "near"],
  aurora: ["sky", "far", "near"],
};
const scene = (slug) => ({ slug, layers: SCENES[slug] });

// A card: 560×388 points of the window (the gallery's 907×627 shape), or
// a closer cut of the same shape (zoomed in: more device pixels a point,
// so it stays as sharp).
const W = 560, H = 388;
const zoomed = (x, y, w) => ({ crop: [x, y, w, (w * H) / W], maxScale: (2 * W) / w });
// zeron/zgx's wallpaper: its layer is the art's box, the window's width
// from the page's right edge (6 + 1164 − 1176 = −6), under the page card's
// clip (the card inside its edge, radius 11), drawn through that branch's
// ASCII.
const ART_BOX = [NODE[0] + NODE[2] - WINDOW.w, NODE[1], WINDOW.w, NODE[3]];
const base = (slug, crop = [0, 0, W, H]) => ({
  window: WINDOW,
  crop,
  scene: scene(slug),
  node: ART_BOX,
  clip: NODE,
  effect: "ascii2",
  page: PAGE,
  radius: CARD_RADIUS - 1,
  pointer: "crop",
  under: drawUnder,
});

// ---------------------------------------------------------------- the cat

// The Composer cat plugin (apps/zgx/plugins-dev/composer-cat, zeron/composer-cat):
// a 28×24 sprite, 2 points a pixel, sampled nearest; its outline's last row
// over the pill's top edge (22 pixels down), 84 in from the composer's right
// edge. The composer (chrome.js) stands at 258, 295.5, 660 wide.
const GRID = [28, 24], PIXEL = 2;
const CAT_AT = [258 + 660 - 84 - GRID[0] * PIXEL, 295.5 - 22 * PIXEL];
const FRAMES = ["idle", "blink", "tail-out", "tail-over", "ear"];
// Its life (init.luau): motions of frames, each on its own timer, one at a
// time.
const BLINK = [["blink", 140]];
const FLICK = [["tail-out", 130], ["idle", 90], ["tail-over", 130], ["idle", 90], ["tail-out", 110]];
const TWITCH = [["ear", 120], ["idle", 80], ["ear", 120]];
const EVERY = [[BLINK, [3500, 7000]], [FLICK, [7000, 12000]], [TWITCH, [10000, 18000]]];
const within = ([a, b]) => a + Math.random() * (b - a);

function loadFrames() {
  return Promise.all(FRAMES.map((name) => new Promise((resolve, reject) => {
    const img = new Image();
    img.onload = () => resolve([name, img]);
    img.onerror = reject;
    img.src = `/assets/hero/cat/cat-${name}.png`;
  }))).then(Object.fromEntries);
}

function catCard() {
  let playing = null, wake = null;
  const timers = [];
  const sprite = { id: "cat", grid: GRID, pixel: PIXEL, at: CAT_AT, frames: null };
  function play(motion) {
    if (playing) return;
    playing = { motion, from: performance.now(), length: motion.reduce((t, [, ms]) => t + ms, 0) };
    wake?.();
  }
  return {
    // (Closer in: the composer's right half, its cat, the model chip, and
    // the field above; cut only where the composer is empty.)
    ...base("wheat"),
    ...zoomed(620, 219, 380),
    label: "A pixel-art cat sitting on the Zeron message box over a golden wheat field drawn in characters",
    over: drawOver,
    sprites: [sprite],
    ready: () => Promise.all([iconsReady(), loadFrames().then((f) => { sprite.frames = f; })]),
    onWake(kick) {
      wake = kick;
      for (const [motion, range] of EVERY) {
        const again = () => { play(motion); timers.push(setTimeout(again, within(range))); };
        timers.push(setTimeout(again, within(range)));
      }
    },
    animate(now) {
      let frame = "idle";
      if (playing) {
        let t = now - playing.from;
        if (t >= playing.length) playing = null;
        else for (const [name, ms] of playing.motion) { if (t < ms) { frame = name; break; } t -= ms; }
      }
      return { poses: { cat: { frame } }, busy: playing !== null };
    },
  };
}

// ---------------------------------------------------------------- machines

// The composer's Work in picker open from its place chip, its Device row's
// machines beside it (invented: this Mac, a Linux devbox, a Mac mini away,
// Cloud from the project's repository), over Icebergs.
function machinesCard() {
  const place = { device: "MacBook Pro", project: "site", branch: "main", open: true };
  const over = (ctx, s) => { drawOver(ctx, s, { place }); drawPopups(ctx, s); };
  return {
    // (Closer in: the device list whole, beside the Work in panel's right
    // edge (its chevrons, the current branch's tag) and under the
    // composer's foot with the model chip; cut where there's no text.)
    ...base("icebergs"),
    ...zoomed(564, 412, 420),
    label: "The Zeron composer's Work in menu open on a list of machines: this MacBook Pro, a Linux devbox, a Mac mini that's offline, and Cloud, over icebergs drawn in characters",
    over,
    ready: () => iconsReady([drawUnder, over]),
  };
}

// ---------------------------------------------------------------- visuals

// A reply's visual alone (show_visual: a title, a legend, a grouped bar
// chart; the visual 340 wide, a column of that width in a row), cut out of
// the session page with 20 around it (a session has no wallpaper: the
// cut-out laid on the new chat's, Aurora, up in the sky, clear of the
// page's ask), rounded as the page card is (12) and shadowed as the
// composer is (0 12 32 #00000066). Whole points from the app's layout, so
// its text falls on the same device-pixel phases.
const CROP_V = zoomed(30, 62, 436);
const VISUAL = { x: 78, y: 106, width: 340, paragraph: false };
const PIECE = [VISUAL.x - 20, VISUAL.y - 20, VISUAL.width + 40, 64 + 150 + 40];
function visualsCard() {
  const over = (ctx, s) => {
    const [x, y, w, h] = PIECE;
    shadowUnder(ctx, s, PIECE);
    ctx.save();
    ctx.beginPath();
    ctx.roundRect(x * s, y * s, w * s, h * s, 12 * s);
    ctx.clip();
    ctx.fillStyle = "rgb(22,22,24)";
    ctx.fillRect(x * s, y * s, w * s, h * s);
    drawReply(ctx, s, VISUAL);
    ctx.restore();
  };
  return {
    ...base("aurora"),
    ...CROP_V,
    label: "A grouped bar chart the Zeron app drew itself in a chat reply, compile, link and test time over six months, over an aurora drawn in characters",
    over,
    ready: () => iconsReady([drawUnder]),
  };
}

export const CARDS = { cat: catCard, machines: machinesCard, visuals: visualsCard };

export function mountCard(container, name, options = {}) {
  const spec = CARDS[name]();
  return mountScene(container, spec, { label: container.getAttribute("data-label") || spec.label, ...options });
}
