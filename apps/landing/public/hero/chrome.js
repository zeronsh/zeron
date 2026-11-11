// The window around the wallpaper, as the Zeron app draws its new chat
// (zeron-dark, opaque; apps/zgx/ui): the title strip, the sidebar, the page
// card, and the composer over the wallpaper. Canvas 2D, in points scaled by
// `s` device pixels a point. Drawn once a size: `drawUnder` (everything the
// wallpaper lies on) and `drawOver` (the composer, with its shadow, over it).
//
// Where each number comes from (file:line in apps/zgx/ui) is in
// tools/native/README.md. Engine facts kept here:
// - colours blend in encoded sRGB, as CSS does; `alpha(c, a)` is a byte;
// - text: line height ceil(1.4·size), baseline top + (line − 1.3·size)/2
//   + 1.005·size; no letter spacing anywhere on this page;
// - an icon is one shape coloured by its tint, its alpha the tint's;
// - opacity multiplies into each shape (the disabled Send: its circle and
//   its arrow each at 0.4, not the button as one picture);
// - a box shadow is the box, offset, covering 0.5·erfc(d / (σ√2)) at signed
//   distance d (σ = blur / 2), under the box too (zgx-gpu draw.wgsl).

import GLYPHS from "./glyphs.js";

export const WINDOW = { w: 1176, h: 768 };
// The page card's radius (zeron.inset.radius); the page clips inside its
// 1-point edge at 11.
export const CARD_RADIUS = 12;

const W = WINDOW.w, H = WINDOW.h;
// The sidebar: closed (its toggle's state; ui/screens/shell/fit.luau). Open,
// the page starts past it (sidebar 261 + 4); closed, at the window's edge.
export const SIDEBAR_OPEN = false;
// The plan's share used, as the ring shows it (invented: the fixture's 38%).
const USED = 0.38;
const PAGE_X = SIDEBAR_OPEN ? 265 : 5;
// The page (the pointer's box), and inside its 1-point edge the wallpaper's
// layer and the page card's clip (radius 11), in points of the window.
export const PAGE = [PAGE_X, 42, W - PAGE_X - 5, H - 47];
export const NODE = [PAGE_X + 1, 43, W - PAGE_X - 7, H - 49];
// (Only the ⌘ is set by the browser: `hint`.)
const SANS = '-apple-system, system-ui, "Arial Unicode MS", sans-serif';

// A colour `rgb` at alpha `byte`/255 over the solid `ground`, as the app's
// surface blends it (encoded sRGB, rounded to a byte): the page fills its
// flat shapes and inks with these, so each lands on the app's byte.
function over(rgb, byte, ground) {
  const a = byte / 255;
  return `rgb(${rgb.map((v, i) => Math.round(ground[i] + (v - ground[i]) * a)).join(",")})`;
}
const TEXT = [237, 237, 239]; // text.primary #ededef
const GROUND = {
  window: [14, 14, 16], // zeron.window #0e0e10
  card: [22, 22, 24], // zeron.inset.bg #ededef09 over the window
  banner: [47, 41, 54], // zeron.tone.accent.wash #9277b033 over the card
  composer: [24, 24, 27], // zeron.composer.solid #18181b
  send: [109, 109, 111], // zeron.primary.bg #ececee at 0.4 over the composer
};
// text.secondary, .tertiary, .disabled: alpha(primary, 0.6 / 0.42 / 0.28).
const ink = (byte, ground) => over(TEXT, byte, GROUND[ground]);
// A colour at alpha `a` (0..1, unrounded: a product of alphas) over a ground
// given as floats, rounded once at the end.
const overF = (rgb, a, ground) => `rgb(${rgb.map((v, i) => Math.round(ground[i] + (v - ground[i]) * a)).join(",")})`;
const mixF = (rgb, a, ground) => rgb.map((v, i) => ground[i] + (v - ground[i]) * a);
const SECONDARY = 153, TERTIARY = 107, DISABLED = 71;
const C = {
  window: "rgb(14,14,16)",
  card: "rgb(22,22,24)",
  banner: "rgb(47,41,54)",
  primary: "rgb(237,237,239)",
  accentInk: "rgb(198,187,254)", // zeron.tone.accent.ink #c6bbfe
  composer: "rgb(24,24,27)",
  composerEdge: over([199, 210, 222], 23, GROUND.composer), // #c7d2de17
  attach: over([255, 255, 255], 20, GROUND.composer), // zeron.secondary.bg #ffffff14
  send: "rgb(109,109,111)",
  sendInk: over([11, 11, 12], 102, GROUND.send), // zeron.primary.text at 0.4
  claude: "rgb(217,119,87)", // the mark's own #D97757
  accent: "rgb(146,119,176)", // accent #9277b0
};

// ---------------------------------------------------------------- icons

// ds/icon_svgs.luau, as the app draws them (currentColor is the tint).
// (A title bar's back and forward, as zeron/zgx draws them: 21 across, their
// lines in the theme's glyph stroke, 1.1667 points: ds/icon.luau
// `in_glyph_stroke`.)
const BAR_STROKE = (1.1667 * 24) / 21;
const ICONS = {
  "bar-left": `<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="${BAR_STROKE}" stroke-linecap="round" stroke-linejoin="round"><path d="M19 12H5m5.5-5.5L5 12l5.5 5.5"/></svg>`,
  "bar-right": `<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="${BAR_STROKE}" stroke-linecap="round" stroke-linejoin="round"><path d="M5 12h14m-5.5-5.5L19 12l-5.5 5.5"/></svg>`,
  monitor: '<svg xmlns="http://www.w3.org/2000/svg" width="1em" height="1em" viewBox="0 0 24 24"><g fill="none" stroke="currentColor" stroke-width="1.5"><path d="M2 10c0-3.771 0-5.657 1.172-6.828S6.229 2 10 2h4c3.771 0 5.657 0 6.828 1.172S22 6.229 22 10v1c0 2.828 0 4.243-.879 5.121C20.243 17 18.828 17 16 17H8c-2.828 0-4.243 0-5.121-.879C2 15.243 2 13.828 2 11z"/><path stroke-linecap="round" d="M16 22H8m4-5v5m10-9H2"/></g></svg>',
  cloud: '<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none"><path d="M7.25 18.5h10.1a4.15 4.15 0 0 0 .45-8.27A6.25 6.25 0 0 0 5.9 8.85a4.85 4.85 0 0 0 1.35 9.65Z" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round"/></svg>',
  "git-branch": '<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.75" stroke-linecap="round" stroke-linejoin="round"><path d="M6 8.5v7M6 12h10a2 2 0 0 0 2-2V8.5"/><circle cx="6" cy="6.25" r="2.25"/><circle cx="6" cy="17.75" r="2.25"/><circle cx="18" cy="6.25" r="2.25"/></svg>',
  "alt-arrow-right": '<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24"><path fill="none" stroke="currentColor" stroke-linecap="round" stroke-linejoin="round" stroke-width="1.5" d="M9 5L15 12L9 19"/></svg>',
  "wifi-off": '<svg xmlns="http://www.w3.org/2000/svg" width="1em" height="1em" viewBox="0 0 24 24"><g fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round"><path d="M2 8.82a15.91 15.91 0 0 1 4.7-3.01M10.66 5.08A15.94 15.94 0 0 1 22 8.82"/><path d="M5.5 12.53a11.02 11.02 0 0 1 2.76-1.87m5.4-1.03a10.96 10.96 0 0 1 4.84 2.9"/><path d="M8.53 16.11A6.48 6.48 0 0 1 12 15c1.7 0 3.26.65 4.42 1.72"/><path d="M12 19.5h.01"/><path d="M3 3l18 18"/></g></svg>',
  plus: '<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.75" stroke-linecap="round" stroke-linejoin="round"><path d="M12 5v14M5 12h14"/></svg>',
  search: '<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.75" stroke-linecap="round" stroke-linejoin="round"><circle cx="10.5" cy="10.5" r="6.5"/><path d="m15.5 15.5 4.5 4.5"/></svg>',
  "arrow-left": '<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.75" stroke-linecap="round" stroke-linejoin="round"><path d="M19 12H5m5.5-5.5L5 12l5.5 5.5"/></svg>',
  "arrow-right": '<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.75" stroke-linecap="round" stroke-linejoin="round"><path d="M5 12h14m-5.5-5.5L19 12l-5.5 5.5"/></svg>',
  "more-horizontal": '<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none"><g fill="currentColor" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round"><circle cx="3.75" cy="12" r="1"/><circle cx="12" cy="12" r="1"/><circle cx="20.25" cy="12" r="1"/></g></svg>',
  "star-bold": '<svg xmlns="http://www.w3.org/2000/svg" width="1em" height="1em" viewBox="0 0 24 24"><path fill="currentColor" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round" d="M12 3.2l2.66 5.39 5.95.86-4.3 4.2 1.01 5.92L12 16.77l-5.32 2.8 1.01-5.92-4.3-4.2 5.95-.86L12 3.2z"/></svg>',
  close: '<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 16 16" fill="none"><path d="m4.75 4.75 6.5 6.5m0-6.5-6.5 6.5" stroke="currentColor" stroke-width="1.25" stroke-linecap="round"/></svg>',
  settings: '<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none"><g stroke="currentColor" stroke-width="1.5" stroke-linejoin="round"><path d="M7.878 5.21415L7.17474 5.05186C6.58003 4.91462 5.95657 5.09343 5.525 5.525C5.09343 5.95657 4.91462 6.58003 5.05186 7.17474L5.21415 7.878C5.40122 8.6886 5.06696 9.53036 4.37477 9.99182L3.51965 10.5619C3.03881 10.8825 2.75 11.4221 2.75 12C2.75 12.5779 3.03881 13.1175 3.51965 13.4381L4.37477 14.0082C5.06696 14.4696 5.40122 15.3114 5.21415 16.122L5.05186 16.8253C4.91462 17.42 5.09343 18.0434 5.525 18.475C5.95657 18.9066 6.58003 19.0854 7.17474 18.9481L7.878 18.7858C8.6886 18.5988 9.53036 18.933 9.99182 19.6252L10.5619 20.4804C10.8825 20.9612 11.4221 21.25 12 21.25C12.5779 21.25 13.1175 20.9612 13.4381 20.4804L14.0082 19.6252C14.4696 18.933 15.3114 18.5988 16.122 18.7858L16.8253 18.9481C17.42 19.0854 18.0434 18.9066 18.475 18.475C18.9066 18.0434 19.0854 17.42 18.9481 16.8253L18.7858 16.122C18.5988 15.3114 18.933 14.4696 19.6252 14.0082L20.4804 13.4381C20.9612 13.1175 21.25 12.5779 21.25 12C21.25 11.4221 20.9612 10.8825 20.4804 10.5619L19.6252 9.99182C18.933 9.53036 18.5988 8.6886 18.7858 7.878L18.9481 7.17473C19.0854 6.58003 18.9066 5.95657 18.475 5.525C18.0434 5.09343 17.42 4.91462 16.8253 5.05186L16.122 5.21415C15.3114 5.40122 14.4696 5.06696 14.0082 4.37477L13.4381 3.51965C13.1175 3.03881 12.5779 2.75 12 2.75C11.4221 2.75 10.8825 3.03881 10.5619 3.51965L9.99182 4.37477C9.53036 5.06696 8.6886 5.40122 7.878 5.21415Z"/><path d="M14.75 12C14.75 13.5188 13.5188 14.75 12 14.75C10.4812 14.75 9.25 13.5188 9.25 12C9.25 10.4812 10.4812 9.25 12 9.25C13.5188 9.25 14.75 10.4812 14.75 12Z"/></g></svg>',
  microphone: '<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24"><g fill="none" stroke="currentColor" stroke-width="1.5"><rect width="7" height="12.5" x="8.5" y="2.25" rx="3.5"/><path stroke-linecap="round" d="M5 11a7 7 0 0 0 14 0M12 18v3.75M9 21.75h6"/></g></svg>',
  "arrow-up": '<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24"><path fill="none" stroke="currentColor" stroke-linecap="round" stroke-linejoin="round" stroke-width="1.5" d="M12 20V4m0 0l6 6m-6-6l-6 6"/></svg>',
  "access-full": '<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none"><path d="M4.75 11.75C4.75 10.6454 5.64543 9.75 6.75 9.75H17.25C18.3546 9.75 19.25 10.6454 19.25 11.75V19.25C19.25 20.3546 18.3546 21.25 17.25 21.25H6.75C5.64543 21.25 4.75 20.3546 4.75 19.25V11.75Z" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round"/><path d="M12 14V17" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round"/><path d="M7.75 9.75V7C7.75 4.65279 9.65279 2.75 12 2.75C14.0926 2.75 15.832 4.26237 16.1847 6.25371" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round"/></svg>',
  worktree: '<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.75" stroke-linecap="round" stroke-linejoin="round"><path d="M6 8.5v7M8.25 6.25H16a2 2 0 0 1 2 2v7.25"/><circle cx="6" cy="6.25" r="2.25"/><circle cx="6" cy="17.75" r="2.25"/><circle cx="18" cy="17.75" r="2.25"/></svg>',
  "alt-arrow-right-bold": '<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24"><path fill="none" stroke="currentColor" stroke-linecap="round" stroke-linejoin="round" stroke-width="2.25" d="M9 5L15 12L9 19"/></svg>',
  claude: '<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 256 257" fill="currentColor"><path d="m50.228 170.321 50.357-28.257.843-2.463-.843-1.361h-2.462l-8.426-.518-28.775-.778-24.952-1.037-24.175-1.296-6.092-1.297L0 125.796l.583-3.759 5.12-3.434 7.324.648 16.202 1.101 24.304 1.685 17.629 1.037 26.118 2.722h4.148l.583-1.685-1.426-1.037-1.101-1.037-25.147-17.045-27.22-18.017-14.258-10.37-7.713-5.25-3.888-4.925-1.685-10.758 7-7.713 9.397.649 2.398.648 9.527 7.323 20.35 15.75L94.817 91.9l3.889 3.24 1.555-1.102.195-.777-1.75-2.917-14.453-26.118-15.425-26.572-6.87-11.018-1.814-6.61c-.648-2.723-1.102-4.991-1.102-7.778l7.972-10.823L71.42 0 82.05 1.426l4.472 3.888 6.61 15.101 10.694 23.786 16.591 32.34 4.861 9.592 2.592 8.879.973 2.722h1.685v-1.556l1.36-18.211 2.528-22.36 2.463-28.776.843-8.1 4.018-9.722 7.971-5.25 6.222 2.981 5.12 7.324-.713 4.73-3.046 19.768-5.962 30.98-3.889 20.739h2.268l2.593-2.593 10.499-13.934 17.628-22.036 7.778-8.749 9.073-9.657 5.833-4.601h11.018l8.1 12.055-3.628 12.443-11.342 14.388-9.398 12.184-13.48 18.147-8.426 14.518.778 1.166 2.01-.194 30.46-6.481 16.462-2.982 19.637-3.37 8.88 4.148.971 4.213-3.5 8.62-20.998 5.184-24.628 4.926-36.682 8.685-.454.324.519.648 16.526 1.555 7.065.389h17.304l32.21 2.398 8.426 5.574 5.055 6.805-.843 5.184-12.962 6.611-17.498-4.148-40.83-9.721-14-3.5h-1.944v1.167l11.666 11.406 21.387 19.314 26.767 24.887 1.36 6.157-3.434 4.86-3.63-.518-23.526-17.693-9.073-7.972-20.545-17.304h-1.36v1.814l4.73 6.935 25.017 37.59 1.296 11.536-1.814 3.76-6.481 2.268-7.13-1.297-14.647-20.544-15.1-23.138-12.185-20.739-1.49.843-7.194 77.448-3.37 3.953-7.778 2.981-6.48-4.925-3.436-7.972 3.435-15.749 4.148-20.544 3.37-16.333 3.046-20.285 1.815-6.74-.13-.454-1.49.194-15.295 20.999-23.267 31.433-18.406 19.702-4.407 1.75-7.648-3.954.713-7.064 4.277-6.286 25.47-32.405 15.36-20.092 9.917-11.6-.065-1.686h-.583L44.07 198.125l-12.055 1.555-5.185-4.86.648-7.972 2.463-2.593 20.35-13.999-.064.065Z"/></svg>',
};

const iconCache = new Map();
// The icon `name` in `color` (opaque: the tint over its ground), decoded.
function iconImage(name, color) {
  const key = name + color;
  if (!iconCache.has(key)) {
    const svg = ICONS[name].replaceAll("currentColor", color);
    const img = new Image();
    img.src = "data:image/svg+xml;charset=utf-8," + encodeURIComponent(svg);
    iconCache.set(key, img.decode().then(() => img));
  }
  return iconCache.get(key);
}

// Every icon this page draws, decoded before the first draw: the page
// drawn once to note them (`wanted`), then each decoded.
const decoded = new Map();
let wanted = null;
export function iconsReady(draws = [drawUnder, drawOver]) {
  wanted = new Set();
  const c = document.createElement("canvas");
  c.width = c.height = 1;
  const ctx = c.getContext("2d");
  for (const draw of draws) draw(ctx, 1);
  const all = [...wanted];
  wanted = null;
  return Promise.all(all.map((key) => {
    const [name, color] = key.split("|");
    return iconImage(name, color).then((img) => decoded.set(key, img));
  }));
}

// An icon `name` fitted ("contain", centred) in the square at x, y, `size`
// points, in `color` (opaque).
function icon(ctx, s, name, x, y, size, color) {
  if (wanted) wanted.add(`${name}|${color}`);
  const img = decoded.get(`${name}|${color}`);
  if (!img) return;
  const vw = img.naturalWidth || size, vh = img.naturalHeight || size;
  const k = size / Math.max(vw, vh);
  const w = vw * k, h = vh * k;
  ctx.drawImage(img, (x + (size - w) / 2) * s, (y + (size - h) / 2) * s, w * s, h * s);
}

// ---------------------------------------------------------------- shapes

function rrect(ctx, s, x, y, w, h, r, fill) {
  ctx.beginPath();
  ctx.roundRect(x * s, y * s, w * s, h * s, Array.isArray(r) ? r.map((v) => v * s) : r * s);
  ctx.fillStyle = fill;
  ctx.fill();
}

// A 1-point edge inside the box (zgx's border: drawn over the box's fill).
function ring(ctx, s, x, y, w, h, r, width, color) {
  ctx.beginPath();
  ctx.roundRect(x * s, y * s, w * s, h * s, r * s);
  ctx.roundRect((x + width) * s, (y + width) * s, (w - 2 * width) * s, (h - 2 * width) * s, Math.max(0, r - width) * s);
  ctx.fillStyle = color;
  ctx.fill("evenodd");
}

// erf, as zgx-gpu's shadow approximates it (draw.wgsl).
function erfApprox(x) {
  const x2 = x * x;
  return Math.sign(x) * Math.sqrt(1 - Math.exp(-x2 * (1.27323954 + 0.147 * x2) / (1 + 0.147 * x2)));
}

// zgx's box shadow: the rounded box (x, y, w, h, r) offset by dx, dy,
// covering 0.5·erfc(d / (σ√2)) at signed distance d, in `rgba`.
function shadow(ctx, s, x, y, w, h, r, dx, dy, blur, [cr, cg, cb, ca]) {
  const sigma = blur / 2;
  const reach = 3 * sigma;
  const bx = x + dx, by = y + dy;
  const x0 = Math.floor((bx - reach) * s), y0 = Math.floor((by - reach) * s);
  const x1 = Math.ceil((bx + w + reach) * s), y1 = Math.ceil((by + h + reach) * s);
  const pw = x1 - x0, ph = y1 - y0;
  const img = ctx.createImageData(pw, ph);
  const hw = w / 2, hh = h / 2, cx = bx + hw, cy = by + hh;
  const rad = Math.min(r, hw, hh);
  for (let j = 0; j < ph; j++) {
    const py = (y0 + j + 0.5) / s;
    const qy = Math.abs(py - cy) - (hh - rad);
    for (let i = 0; i < pw; i++) {
      const px = (x0 + i + 0.5) / s;
      const qx = Math.abs(px - cx) - (hw - rad);
      const d = Math.hypot(Math.max(qx, 0), Math.max(qy, 0)) + Math.min(Math.max(qx, qy), 0) - rad;
      const cover = 0.5 - 0.5 * erfApprox(d / (sigma * Math.SQRT2));
      const o = (j * pw + i) * 4;
      img.data[o] = cr; img.data[o + 1] = cg; img.data[o + 2] = cb;
      img.data[o + 3] = Math.round(ca * cover * 255);
    }
  }
  // (Composited over what's drawn, not put: through a canvas of its own.)
  const c = document.createElement("canvas");
  c.width = pw; c.height = ph;
  c.getContext("2d").putImageData(img, 0, 0);
  ctx.drawImage(c, x0, y0);
}

// ---------------------------------------------------------------- text

const lineOf = (size) => Math.ceil(size * 1.4);
const paths = new Map();
function glyphPath(key) {
  if (!paths.has(key)) paths.set(key, new Path2D(GLYPHS.glyphs[key]));
  return paths.get(key);
}
// A run's advance in points (its shaped glyphs'), or the browser's
// measure for words not in glyphs.js.
function advanceOf(ctx, s, str, size, weight) {
  const run = GLYPHS.runs[`${weight}|${str}`];
  if (run) return run.advance * size / GLYPHS.upm;
  ctx.font = `${weight} ${size * s}px ${SANS}`;
  return ctx.measureText(str).width / s;
}
// Text at `x` (its left, or its right when `right`), its line box's top at
// `top`; one line. Geist's words are filled from its outlines (glyphs.js);
// anything else (the ⌘, which Geist doesn't have and the app takes from
// the system's fallback too) is set by the browser.
function text(ctx, s, str, x, top, size, weight, color, { right = false, line = lineOf(size) } = {}) {
  // Placed as cosmic-text places a glyph (layout.rs `physical`): across, at
  // its box's left plus its pen position binned to a quarter device pixel
  // (`SubpixelBin`: the nearest quarter); down, its box's top plus the
  // baseline's offset in the box on a whole device pixel ("hinting in Y";
  // the app's pixels say nearest). Below 2x the app puts glyphs on whole
  // pixels.
  const local = (line - 1.3 * size) / 2 + 1.005 * size;
  const baseline = top * s + Math.round(local * s);
  const width = advanceOf(ctx, s, str, size, weight);
  const left = right ? x - width : x;
  const run = GLYPHS.runs[`${weight}|${str}`];
  ctx.fillStyle = color;
  if (run) {
    const k = size / GLYPHS.upm;
    const bin = s < 2 ? 1 : 4;
    // (On whatever the context is translated by: a card's crop.)
    const at = ctx.getTransform();
    for (const [key, gx, gy] of run.glyphs) {
      const gxp = left * s + Math.round(gx * k * s * bin) / bin;
      ctx.save();
      ctx.setTransform(at.multiply(new DOMMatrix([s * k, 0, 0, -s * k, gxp, baseline - gy * k * s])));
      ctx.fill(glyphPath(key));
      ctx.restore();
    }
  } else {
    ctx.font = `${weight} ${size * s}px ${SANS}`;
    ctx.textBaseline = "alphabetic";
    ctx.textAlign = "left";
    ctx.fillText(str, left * s, baseline);
  }
  return width;
}

// A shortcut hint ("⌘N"): the letter Geist's, the pair ending at `x`. Geist
// has no ⌘: the app's fallback list puts .SF NS first, and draws SF's ⌘ (its
// default instance). SF isn't the web's to ship, so the ⌘ here is the
// system's own (SF on a Mac), its ink fitted to SF's glyph box (SFNS.ttf:
// advance 0.919 em, ink 0.098 to 0.821 across, −0.007 to 0.714 up).
const CMD = { adv: 0.9189, minX: 0.0977, maxX: 0.8213, minY: -0.0073, maxY: 0.7144 };
function hint(ctx, s, key, x, top, color) {
  const letter = advanceOf(ctx, s, key, 11, 400);
  text(ctx, s, key, x, top, 11, 400, color, { right: true });
  const size = 11, line = lineOf(size);
  const baseline = top * s + Math.round(((line - 1.3 * size) / 2 + 1.005 * size) * s);
  const pen = x - letter - CMD.adv * size;
  const box = [(pen + CMD.minX * size) * s, baseline - CMD.maxY * size * s, (CMD.maxX - CMD.minX) * size * s, (CMD.maxY - CMD.minY) * size * s];
  ctx.font = `400 ${100}px ${SANS}`;
  const m = ctx.measureText("⌘");
  const iw = m.actualBoundingBoxLeft + m.actualBoundingBoxRight, ih = m.actualBoundingBoxAscent + m.actualBoundingBoxDescent;
  if (!(iw > 0 && ih > 0)) return;
  ctx.save();
  ctx.translate(box[0], box[1]);
  ctx.scale(box[2] / iw, box[3] / ih);
  ctx.fillStyle = color;
  ctx.textBaseline = "alphabetic";
  ctx.textAlign = "left";
  ctx.fillText("⌘", m.actualBoundingBoxLeft, m.actualBoundingBoxAscent);
  ctx.restore();
}

// ---------------------------------------------------------------- pieces

// ds/sidebar_icon.luau: a window frame with its sidebar panel, drawn as
// boxes (16-point glyph at x, y; the panel at the left or right).
function sidebarGlyph(ctx, s, x, y, side, open, color) {
  ring(ctx, s, x + 1.416667, y + 2.083333, 13.166667, 11.833333, 3.25, 1.1667, color);
  const pw = 1.1667 + (open ? 2.5 : 0);
  const px = side === "left" ? x + 4.333333 : x + 16 - 4.333333 - pw;
  rrect(ctx, s, px, y + 5, pw, 6, 0.583333, color);
}

// ds/avatar.luau: a signed-out account's waves (seed "Local").
function avatar(ctx, s, x, y, size) {
  const c0 = [198, 187, 254], c1 = [169, 113, 227], c2 = [94, 106, 213];
  const P = [0.05655021080747247, 0.475712280953303, 0.2205217694863677, 0.23442226904444396];
  const n = Math.ceil(size * s);
  const x0 = Math.round(x * s), y0 = Math.round(y * s);
  const img = ctx.createImageData(n + 2, n + 2);
  const sm = (a, b, v) => { const t = Math.min(Math.max((v - a) / (b - a), 0), 1); return t * t * (3 - 2 * t); };
  const TAU = Math.PI * 2;
  for (let j = 0; j < n + 2; j++) {
    for (let i = 0; i < n + 2; i++) {
      const px = (x0 + i + 0.5) / s - x, py = (y0 + j + 0.5) / s - y;
      const ux = px / size - 0.5, uy = py / size - 0.5;
      const a = P[0] * TAU;
      const rx = Math.cos(a) * ux - Math.sin(a) * uy, ry = Math.sin(a) * ux + Math.cos(a) * uy;
      const k = 2.2 + P[1] * 2.4;
      const t1 = 0.5 + 0.5 * Math.sin(rx * k + P[2] * TAU + 1.6 * Math.sin(ry * k * 1.3 + P[3] * TAU));
      const t2 = 0.5 + 0.5 * Math.sin(ry * k * 0.9 - P[3] * TAU + 1.3 * Math.sin(rx * k * 1.1 + P[2] * TAU));
      const m1 = sm(0.15, 0.85, t1), m2 = 0.65 * sm(0.3, 0.95, t2);
      // The circle's coverage, its edge antialiased a device pixel wide.
      const d = Math.hypot(px - size / 2, py - size / 2) - size / 2;
      const cover = Math.min(Math.max(0.5 - d * s, 0), 1);
      const o = (j * (n + 2) + i) * 4;
      for (let ch = 0; ch < 3; ch++) {
        const v = c0[ch] + (c1[ch] - c0[ch]) * m1;
        img.data[o + ch] = Math.round(v + (c2[ch] - v) * m2);
      }
      img.data[o + 3] = Math.round(cover * 255);
    }
  }
  const c = document.createElement("canvas");
  c.width = n + 2; c.height = n + 2;
  c.getContext("2d").putImageData(img, 0, 0);
  ctx.drawImage(c, x0, y0);
}

// macOS's window controls (native in the app: not in its own pictures):
// 14-point lights centred 21 in and 23 apart, on the bar's line, 20 down
// (ds/title_bar.luau's LIGHTS_LINE on zeron/zgx).
function trafficLights(ctx, s) {
  const lights = [["#ff5f57", "#e0443e"], ["#febc2e", "#dea123"], ["#28c840", "#1aab29"]];
  lights.forEach(([fill, edge], i) => {
    const cx = 21 + i * 23, cy = 20;
    ctx.beginPath();
    ctx.arc(cx * s, cy * s, 7 * s, 0, Math.PI * 2);
    ctx.fillStyle = fill;
    ctx.fill();
    ctx.lineWidth = 0.5 * s;
    ctx.strokeStyle = edge;
    ctx.beginPath();
    ctx.arc(cx * s, cy * s, 6.75 * s, 0, Math.PI * 2);
    ctx.stroke();
  });
}

// ---------------------------------------------------------------- layers

// Everything the wallpaper lies on.
export function drawUnder(ctx, s, { lights = true } = {}) {
  ctx.fillStyle = C.window;
  ctx.fillRect(0, 0, ctx.canvas.width, ctx.canvas.height);
  if (lights) trafficLights(ctx, s);

  // The title strip's controls (ds/title_bar.luau): 24-point buttons on
  // the line 21 down, their 16-point glyphs inside.
  // (zeron/zgx: the row on the lights' line, 20 down; the bar's ink
  // #949496; back and forward 21 across in the glyph stroke. The sidebar's
  // view options, "…", go with the sidebar.)
  sidebarGlyph(ctx, s, 92, 12, "left", SIDEBAR_OPEN, ink(SECONDARY, "window"));
  icon(ctx, s, "bar-left", 121.5, 9.5, 21, ink(DISABLED, "window"));
  icon(ctx, s, "bar-right", 147.5, 9.5, 21, ink(DISABLED, "window"));
  if (SIDEBAR_OPEN) icon(ctx, s, "more-horizontal", 182, 12, 16, ink(SECONDARY, "window"));
  sidebarGlyph(ctx, s, W - 30, 12, "right", false, ink(SECONDARY, "window"));

  if (SIDEBAR_OPEN) sidebar(ctx, s);

  // The page card's inside (its 1-point edge is clipped away): the
  // wallpaper's ground where the wallpaper is clear.
  rrect(ctx, s, NODE[0], NODE[1], NODE[2], NODE[3], CARD_RADIUS - 1, C.card);
}

// The sidebar, open (ui/ds/sidebar.luau, screens/shell/sidebar.luau).
function sidebar(ctx, s) {
  // Its card.
  rrect(ctx, s, 5, 42, 256, H - 47, 12, C.card);
  // New chat, Search.
  icon(ctx, s, "plus", 20, 56.5, 16, ink(SECONDARY, "card"));
  text(ctx, s, "New chat", 43, 55.5, 13, 500, C.primary);
  hint(ctx, s, "N", 245, 57, ink(TERTIARY, "card"));
  icon(ctx, s, "search", 20, 88.5, 16, ink(SECONDARY, "card"));
  text(ctx, s, "Search", 43, 87.5, 13, 400, C.primary);
  hint(ctx, s, "K", 245, 89, ink(TERTIARY, "card"));
  // No sessions yet; the Sessions header.
  text(ctx, s, "No sessions yet", 21, 130, 11, 400, ink(TERTIARY, "card"));
  text(ctx, s, "Sessions", 21, 158, 11, 500, ink(TERTIARY, "card"));
  // Star on GitHub.
  rrect(ctx, s, 13, H - 75, 240, 30, 8, C.banner);
  icon(ctx, s, "star-bold", 21, H - 65.5, 12, C.accentInk);
  text(ctx, s, "Star on GitHub", 39, H - 68, 11, 500, C.accentInk);
  icon(ctx, s, "close", 231, H - 67, 14, ink(TERTIARY, "banner"));
  // The account: Local, Settings.
  avatar(ctx, s, 18, H - 38.5, 20);
  text(ctx, s, "Local", 43, H - 37.5, 13, 500, C.primary);
  icon(ctx, s, "settings", 230, H - 36, 16, ink(TERTIARY, "card"));
}

// The model chip's usage ring (ds/context_usage.luau `Ring`, as the model
// picker's trail): 12 across, a 1.5-point track (border.strong) of radius
// 5.25, and the plan's used share from twelve o'clock, clockwise, round
// capped, in the accent (amber from 80%, red from 95%).
function usageRing(ctx, s, x, y, used) {
  const cx = (x + 6) * s, cy = (y + 6) * s, r = 5.25 * s;
  ctx.save();
  ctx.lineWidth = 1.5 * s;
  ctx.strokeStyle = over([255, 255, 255], 46, GROUND.composer);
  ctx.beginPath();
  ctx.arc(cx, cy, r, 0, Math.PI * 2);
  ctx.stroke();
  if (used > 0) {
    ctx.strokeStyle = used >= 0.95 ? "rgb(239,68,68)" : used >= 0.8 ? "rgb(245,158,11)" : C.accent;
    ctx.lineCap = "round";
    ctx.beginPath();
    ctx.arc(cx, cy, r, -Math.PI / 2, -Math.PI / 2 + Math.PI * 2 * used);
    ctx.stroke();
  }
  ctx.restore();
}

// The composer, over the wallpaper (UI/ds/composer.luau; a new chat with
// the wallpaper showing: the pill, the tray behind its footer).
// `place`: what the place chip says, and whether its picker is open (the
// chip washed `hover` then). Default: a first-time user's new chat.
export function drawOver(ctx, s, { place = { device: "MacBook Pro", project: "hello-world", branch: "No ref", open: false } } = {}) {
  const pane = NODE;
  const cw = Math.min(pane[2] - 64, 660);
  const cx = pane[0] + 32 + (pane[2] - 64 - cw) / 2;
  const cy = pane[1] + pane[3] / 2 - 107;
  const right = cx + cw;

  // The tray: 76 tall from the composer's foot, its top tucked 40 behind
  // the pill, rounded 16 at its foot.
  rrect(ctx, s, cx, cy + 80, cw, 76, [0, 0, 16, 16], C.composer);
  // The pill: its shadow (0 12 32 #00000066), its fill, its 1-point edge.
  shadow(ctx, s, cx, cy, cw, 120, 26, 0, 12, 32, [0, 0, 0, 0.4]);
  rrect(ctx, s, cx, cy, cw, 120, 26, C.composer);
  ring(ctx, s, cx, cy, cw, 120, 26, 1, C.composerEdge);

  // The placeholder.
  text(ctx, s, "Ask anything, @ to mention…", cx + 17, cy + 17, 14, 400, ink(TERTIARY, "composer"), { line: 22 });
  // Attach.
  rrect(ctx, s, cx + 12, cy + 80, 28, 28, 14, C.attach);
  icon(ctx, s, "plus", cx + 17, cy + 85, 18, C.primary);
  // Dictate.
  icon(ctx, s, "microphone", right - 71, cy + 85, 18, ink(SECONDARY, "composer"));
  // Send, disabled (0.4 on each of its shapes).
  rrect(ctx, s, right - 40, cy + 80, 28, 28, 14, C.send);
  icon(ctx, s, "arrow-up", right - 33, cy + 87, 14, C.sendInk);

  // The footer: three chips, words 12 Medium tertiary, 1 above centre.
  const ty = cy + 126;
  const label = (str, x) => text(ctx, s, str, x, ty + 2.5, 12, 500, ink(TERTIARY, "composer"));
  // Full access.
  icon(ctx, s, "access-full", cx + 18, ty + 6, 12, ink(TERTIARY, "composer"));
  label("Full access", cx + 36);
  // MacBook Pro › hello-world › No ref. (Each name a FadeText whose box,
  // the app's pixels show, takes its run's advance to the whole point.)
  const words = [place.device, place.project, place.branch].map((w) => Math.round(advanceOf(ctx, s, w, 12, 500)));
  if (place.open) {
    // Open: the chip washed as under the pointer (over the tray, the pill's
    // shadow on it: the wash's alpha laid on what's drawn).
    const w = 6 + 12 + 6 + words[0] + 4 + 10 + 6 + words[1] + 6 + 10 + 4 + words[2] + 6;
    rrect(ctx, s, cx + 109, ty, w, 24, 8, "rgba(255,255,255,0.051)");
  }
  let x = cx + 109 + 6;
  icon(ctx, s, "worktree", x, ty + 6, 12, ink(TERTIARY, "composer"));
  x += 18;
  label(place.device, x);
  x += words[0] + 4;
  icon(ctx, s, "alt-arrow-right-bold", x, ty + 7, 10, ink(DISABLED, "composer"));
  x += 10 + 6;
  label(place.project, x);
  x += words[1] + 6;
  icon(ctx, s, "alt-arrow-right-bold", x, ty + 7, 10, ink(DISABLED, "composer"));
  x += 14;
  label(place.branch, x);
  // The model (ds/model_picker.luau, a chip): Claude's mark in its own
  // colour, the model, its effort (the detail, a step quieter), the usage
  // ring; 6 apart and 6 in. (Its words' boxes to the whole point, as the
  // place chip's: the app's tree has the chip 130 wide.)
  const model = Math.round(advanceOf(ctx, s, "Opus 5.5", 12, 500));
  const effort = Math.round(advanceOf(ctx, s, "High", 12, 500));
  const chip = right - 12 - (6 + 12 + 6 + model + 6 + effort + 6 + 12 + 6);
  icon(ctx, s, "claude", chip + 6, ty + 6, 12, C.claude);
  label("Opus 5.5", chip + 24);
  text(ctx, s, "High", chip + 24 + model + 6, ty + 2.5, 12, 500, ink(DISABLED, "composer"));
  usageRing(ctx, s, chip + 24 + model + 6 + effort + 6, ty + 6, USED);
}

// ---------------------------------------------------------------- popups

// The composer's Work in picker open, and its Device row's machines beside
// it (UI/ds/composer.luau `work_in`, `submenu`; ds/dropdown.luau; ds/menu.luau):
// two popup windows, each a menu panel (zeron.menu.solid #1c1c1f, a 1-point
// border #ffffff12 inside its edge, radius 8, its own shadow 0 20 56
// #00000052, which the window's box cuts: only its corners show it).
const PANEL = [28, 28, 31];
const HOVER = mixF([255, 255, 255], 13 / 255, PANEL); // #ffffff0d
const SELECTED = mixF([235, 235, 235], 23 / 255, PANEL); // zeron.menu.selected #ebebeb17
const T = TEXT;
const inkOn = (a, ground) => overF(T, a, ground);

function panel(ctx, s, x, y, w, h) {
  ctx.save();
  ctx.beginPath();
  ctx.rect(x * s, y * s, w * s, h * s);
  ctx.clip();
  shadow(ctx, s, x, y, w, h, 8, 0, 20, 56, [0, 0, 0, 82 / 255]);
  ctx.restore();
  rrect(ctx, s, x, y, w, h, 8, "rgb(28,28,31)");
  ring(ctx, s, x, y, w, h, 8, 1, over([255, 255, 255], 18, PANEL));
}

// A row's glyph (16 in an 18 box at x+8), label (Geist 13, line 19) and
// what's at its end; `ground` what it lies on, `a` its opacity.
function row(ctx, s, x, y, w, { icon: name, tile, label, tag, chevron, ground = PANEL, a = 1 }) {
  if (name) icon(ctx, s, name, x + 9, y + 6, 16, inkOn(0.6 * a, ground));
  if (tile) {
    rrect(ctx, s, x + 8, y + 5, 18, 18, 4, "rgb(24,170,153)");
    const lw = advanceOf(ctx, s, tile, 11, 600);
    text(ctx, s, tile, x + 8 + (18 - lw) / 2, y + 6, 11, 600, "rgb(239,250,247)");
  }
  text(ctx, s, label, x + 34, y + 4.5, 13, 400, inkOn(a, ground));
  if (tag) text(ctx, s, tag, x + w - 8, y + 6, 11, 400, inkOn((107 / 255) * a, ground), { right: true });
  if (chevron) icon(ctx, s, "alt-arrow-right", x + w - 22, y + 7, 14, inkOn(107 / 255, ground));
}

// A search field's row: its glyph, its placeholder, and (focused) the caret,
// 1 by 19 points at the text's start, shown (it blinks 650/350 ms).
function search(ctx, s, x, y, words, focused) {
  icon(ctx, s, "search", x + 12, y + 11, 14, inkOn(107 / 255, PANEL));
  text(ctx, s, words, x + 34, y + 8.5, 13, 400, inkOn(107 / 255, PANEL));
  if (focused) {
    ctx.fillStyle = inkOn(107 / 255, PANEL);
    ctx.fillRect((x + 34) * s, (y + 8.5) * s, 1 * s, 19 * s);
  }
}

const rule = (ctx, s, x, y, w) => { ctx.fillStyle = over([255, 255, 255], 10, PANEL); ctx.fillRect(x * s, y * s, w * s, 1 * s); };

export function drawPopups(ctx, s) {
  // Work in (280×252 at 367, 452).
  let X = 367, Y = 452;
  panel(ctx, s, X, Y, 280, 252);
  text(ctx, s, "Work in", X + 13, Y + 9, 11, 500, inkOn(107 / 255, PANEL));
  rrect(ctx, s, X + 5, Y + 29, 270, 28, 3, `rgb(${HOVER.map(Math.round).join(",")})`);
  row(ctx, s, X + 5, Y + 29, 270, { icon: "monitor", label: "MacBook Pro", chevron: true, ground: HOVER });
  row(ctx, s, X + 5, Y + 57, 270, { tile: "S", label: "site", chevron: true });
  row(ctx, s, X + 5, Y + 85, 270, { icon: "worktree", label: "Worktree", chevron: true });
  rule(ctx, s, X + 1, Y + 117, 278);
  search(ctx, s, X + 1, Y + 118, "Search branches…", false);
  rule(ctx, s, X + 1, Y + 154, 278);
  rrect(ctx, s, X + 5, Y + 159, 270, 28, 3, `rgb(${SELECTED.map(Math.round).join(",")})`);
  row(ctx, s, X + 5, Y + 159, 270, { icon: "git-branch", label: "main", tag: "Current", ground: SELECTED });
  row(ctx, s, X + 5, Y + 189, 270, { icon: "git-branch", label: "fix/flaky-tests" });
  row(ctx, s, X + 5, Y + 219, 270, { icon: "git-branch", label: "feat/cache-builds" });

  // The Device menu (260×213 at 646, 481).
  X = 646; Y = 481;
  panel(ctx, s, X, Y, 260, 213);
  search(ctx, s, X + 1, Y + 1, "Search devices…", true);
  rule(ctx, s, X + 1, Y + 37, 258);
  rrect(ctx, s, X + 5, Y + 42, 250, 28, 3, `rgb(${SELECTED.map(Math.round).join(",")})`);
  row(ctx, s, X + 5, Y + 42, 250, { icon: "monitor", label: "MacBook Pro", tag: "You", ground: SELECTED });
  row(ctx, s, X + 5, Y + 72, 250, { icon: "monitor", label: "devbox" });
  // Mac mini, offline: the whole row at 0.45, its two lines, the warning.
  const dim = 0.45;
  icon(ctx, s, "monitor", X + 14, Y + 111, 16, inkOn(0.6 * dim, PANEL));
  text(ctx, s, "Mac mini", X + 39, Y + 110, 13, 500, inkOn(dim, PANEL), { line: 18 });
  text(ctx, s, "Offline · last seen 3 h ago", X + 39, Y + 130, 12, 400, inkOn((107 / 255) * dim, PANEL), { line: 16 });
  icon(ctx, s, "wifi-off", X + 235, Y + 122, 12, overF([250, 204, 21], dim, PANEL));
  // Cloud.
  icon(ctx, s, "cloud", X + 14, Y + 165, 16, inkOn(0.6, PANEL));
  text(ctx, s, "Cloud", X + 39, Y + 164, 13, 500, inkOn(1, PANEL), { line: 18 });
  text(ctx, s, "A machine of its own, from acme/site", X + 39, Y + 184, 12, 400, inkOn(107 / 255, PANEL), { line: 16 });
}

// ---------------------------------------------------------------- a reply

// An assistant's reply with a visual, as a session's transcript draws it
// (UI/ds/transcript.luau, ds/markdown.luau, ds/visual.luau, ds/chart.luau;
// the conversation 440 wide: its column 356..820): a paragraph, then the
// visual (show_visual, run by the app's host): a title, a legend and a
// grouped bar chart. The chart's labels are the canvas's default face, the
// system's sans (SF on a Mac), not Geist.
const CARD_GROUND = [22, 22, 24];
const SERIES = [["Compile", [146, 119, 176], [42, 38, 51, 47, 33, 29]], ["Link", [88, 194, 178], [12, 14, 11, 16, 9, 8]], ["Test", [233, 166, 75], [20, 22, 19, 25, 18, 21]]];
const MONTHS = ["Jun", "Jul", "Aug", "Sep", "Oct", "Nov"];

export function drawReply(ctx, s) {
  const X = 356;
  const primary = "rgb(237,237,239)";
  // The paragraph: Geist 15/24, its lines as the app breaks them at 464.
  ["Compile is still the biggest share, but it has dropped since August,", "when the workspace was split. Here are the last six months, by", "step:"]
    .forEach((line, i) => text(ctx, s, line, X, 151 + 24 * i, 15, 400, primary, { line: 24 }));
  // The visual: its title (SemiBold 16/24), 12 below it the legend (8-point
  // dots, Geist 13/20 in text.secondary, 6 from the dot, 16 between).
  text(ctx, s, "Where the build's time goes", X, 235, 16, 600, primary, { line: 24 });
  let lx = X;
  for (const [name, rgb] of SERIES) {
    ctx.beginPath();
    ctx.arc((lx + 4) * s, 281 * s, 4 * s, 0, Math.PI * 2);
    ctx.fillStyle = `rgb(${rgb.join(",")})`;
    ctx.fill();
    const w = Math.round(advanceOf(ctx, s, name, 13, 400));
    text(ctx, s, name, lx + 14, 271, 13, 400, overF(T, 153 / 255, CARD_GROUND), { line: 20 });
    lx += 14 + w + 16;
  }
  // The plot (a canvas 464×150 at 356, 299): insets top 8, right 6, bottom
  // 22, left the widest label + 8 (32.93: the app's label face measured);
  // ticks 0..60 by 10 (nice_ticks over 0..51).
  const top = 299 + 8, left = X + 32.93, pw = 464 - 32.93 - 6, ph = 120;
  const vy = (v) => top + ph - (v / 60) * ph;
  for (let t = 0; t <= 60; t += 10) {
    ctx.fillStyle = over([255, 255, 255], t === 0 ? 18 : 10, CARD_GROUND);
    ctx.fillRect(left * s, (vy(t) - 0.5) * s, pw * s, 1 * s);
  }
  const band = pw / 6, group = band * 0.68, per = group / 3, gap = Math.min(2, per * 0.15), bw = per - gap;
  MONTHS.forEach((_, i) => SERIES.forEach(([, rgb, values], si) => {
    const x0 = left + i * band + (band - group) / 2 + si * per + gap / 2;
    const y0 = vy(values[i]);
    rrect(ctx, s, x0, y0, bw, vy(0) - y0, 3, `rgb(${rgb.join(",")})`);
  }));
  // Labels: 12, text.tertiary; the values' right-aligned 8 short of the
  // plot, centred on their line; the months centred under their bands.
  const tertiary = overF(T, 107 / 255, CARD_GROUND);
  // (Each value's start as the app lays it, right-aligned by the app's
  // label face's widths: "0 s" 17.21, the rest 24.93. The web's system face
  // measures a little otherwise, so it's placed, not aligned.)
  for (let t = 0; t <= 60; t += 10) text(ctx, s, `${t} s`, left - 8 - (t === 0 ? 17.21 : 24.93), vy(t) - 8.5, 12, 400, tertiary);
  MONTHS.forEach((m, i) => {
    const w = advanceOf(ctx, s, m, 12, 400);
    text(ctx, s, m, left + (i + 0.5) * band - w / 2, top + ph + 6, 12, 400, tertiary);
  });
}

// The composer's shadow (0 12 32 #00000066) under a rounded box (radius 12):
// a cut-out's, on a card.
export function shadowUnder(ctx, s, [x, y, w, h]) {
  shadow(ctx, s, x, y, w, h, 12, 0, 12, 32, [0, 0, 0, 0.4]);
}
