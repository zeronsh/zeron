// The hero: the Zeron app's new chat, redrawn for the web from the app's own
// numbers (apps/zgx). A canvas: the window's chrome in Canvas 2D (chrome.js),
// the wallpaper and its effect in WebGL2 (wallpaper.js). Only the wallpaper
// moves, so the chrome is drawn once a size and each frame redraws only the
// wallpaper, and only while its springs move.
//
// mountHero(container, { still, pose, dpr, lights }) → controller, or null
// when WebGL2 isn't there (the page keeps its still picture then). `still`
// (or Reduce motion): the scene holds still and nothing follows the pointer.

import { Renderer, texture } from "./wallpaper.js";
import { WINDOW, CARD_RADIUS, drawUnder, drawOver, iconsReady } from "./chrome.js";

// The Parallax scenes plugin's numbers (init.luau): a step of depth moves
// 14 points across and 9 down at the pointer's edge, at Normal; the springs
// are Zeron's (stiffness 140, damping 20).
const STEP_X = 14, STEP_Y = 9;
const SPRING = { stiffness: 140, damping: 20 };
const LAYERS = ["sky", "far", "near", "detail"];
const ART = { w: 576, h: 768 };

// Where things stand, in points of the window (the app's tree at 1176×768):
// the page (the pointer's box), the wallpaper's layer (inside the page
// card's 1-point edge), the place a plugin draws in (laid at the window's
// width from the layer's right edge), the page card's inside clip.
const PAGE = [265, 42, 906, 721];
const NODE = [266, 43, 904, 719];
const PLACE = [NODE[0] + NODE[2] - WINDOW.w, NODE[1], WINDOW.w, NODE[3]];
// ds/wallpaper.luau: ASCII at a 1.5-point font pixel, Medium strength
// (0.75) and density (no glyph under 0.22).
const ASCII = [1.5, 0.75, 0.22];

// zgx's spring (zgx-script anim.rs): semi-implicit Euler, settling within a
// thousandth. Stepped at the app's 120 Hz (`frame`).
class Spring {
  constructor(value) { this.x = value; this.goal = value; this.v = 0; }
  settled() { return this.x === this.goal && this.v === 0; }
  step(dt) {
    if (this.settled()) return;
    dt = Math.min(dt, 1 / 30);
    const to = this.goal;
    this.v += (-SPRING.stiffness * (this.x - to) - SPRING.damping * this.v) * dt;
    const x = this.x + this.v * dt;
    const settle = 1e-3 * Math.max(Math.abs(to), 1);
    if (!Number.isFinite(x) || (Math.abs(x - to) < settle && Math.abs(this.v) < settle * 10)) {
      this.x = to; this.v = 0;
    } else {
      this.x = x;
    }
  }
}

function loadImage(src) {
  return new Promise((resolve, reject) => {
    const img = new Image();
    img.decoding = "async";
    img.onload = () => resolve(img);
    img.onerror = () => reject(new Error(`no picture: ${src}`));
    img.src = src;
  });
}

export async function mountHero(container, options = {}) {
  const base = options.base ?? "/assets/hero/blossom/";
  const reduced = options.still ?? matchMedia("(prefers-reduced-motion: reduce)").matches;
  const canvas = document.createElement("canvas");
  canvas.className = "hero-app-canvas";
  canvas.setAttribute("role", "img");
  canvas.setAttribute("aria-label", container.getAttribute("data-label") || "The Zeron app: a new chat over a cherry blossom scene drawn in characters");
  const gl = canvas.getContext("webgl2", { alpha: false, antialias: false, premultipliedAlpha: true, preserveDrawingBuffer: !!options.capture });
  if (!gl) return null;
  const renderer = new Renderer(gl);
  const [images] = await Promise.all([Promise.all(LAYERS.map((l) => loadImage(base + l + ".webp"))), iconsReady()]);
  const pictures = images.map((img) => ({ tex: texture(gl, img), w: ART.w, h: ART.h }));
  // The reach: Normal (1); Reduce motion holds the scene still (0), as the
  // plugin does.
  const reach = reduced ? 0 : 1;
  const deepest = LAYERS.length - 1;
  const over = [Math.ceil(deepest * STEP_X * reach), Math.ceil(deepest * STEP_Y * reach)];
  const springs = LAYERS.map(() => [new Spring(0), new Spring(0)]);

  const STEP = 1 / 120;
  let acc = 0;
  let scale = 1, cw = 0, ch = 0, under = null, overTex = null, dirty = true, raf = 0, visible = true, last = 0;

  function resize() {
    const dpr = options.dpr ?? window.devicePixelRatio ?? 1;
    const css = container.clientWidth || WINDOW.w;
    // (At most the app's own 2x: past it the browser scales the canvas.)
    scale = Math.min((css / WINDOW.w) * dpr, 2);
    const w = Math.round(WINDOW.w * scale), h = Math.round(WINDOW.h * scale);
    if (w === cw && h === ch) return;
    cw = w; ch = h;
    canvas.width = w; canvas.height = h;
    const paint = (draw) => {
      const c = document.createElement("canvas");
      c.width = w; c.height = h;
      // (On the CPU: analytic coverage, as the app's; a GPU canvas's 4× MSAA steps edges in quarters.)
      const ctx = c.getContext("2d", { willReadFrequently: true });
      draw(ctx, scale);
      return c;
    };
    if (under) gl.deleteTexture(under);
    if (overTex) gl.deleteTexture(overTex);
    under = texture(gl, paint((ctx, k) => drawUnder(ctx, k, { lights: options.lights !== false })));
    overTex = texture(gl, paint(drawOver));
    dirty = true;
  }

  function draw() {
    gl.bindFramebuffer(gl.FRAMEBUFFER, null);
    gl.viewport(0, 0, cw, ch);
    gl.disable(gl.BLEND);
    gl.clearColor(0, 0, 0, 1);
    gl.clear(gl.COLOR_BUFFER_BIT);
    const boxes = pictures.map((pic, i) => ({
      ...pic,
      box: [PLACE[0] - over[0] + springs[i][0].x, PLACE[1] - over[1] + springs[i][1].x, PLACE[2] + 2 * over[0], PLACE[3] + 2 * over[1]],
    }));
    renderer.drawLayer(scale, NODE, boxes, 0);
    gl.viewport(0, 0, cw, ch);
    renderer.drawFull(under, cw, ch);
    renderer.drawEffect(cw, ch, scale, NODE, ASCII, NODE, CARD_RADIUS - 1, options.effect === "none" ? 0 : 1, NODE[3]);
    renderer.drawFull(overTex, cw, ch);
  }

  function frame(now) {
    raf = 0;
    // The app steps its springs at its 120 Hz frames: stepped here in the
    // same 1/120 s steps, whatever this display's rate, the layers pass
    // through the app's positions (tools/native: the tree's boxes over time
    // match this to the point).
    acc += Math.min(last ? (now - last) / 1000 : STEP, 0.25);
    last = now;
    while (acc >= STEP) {
      for (const pair of springs) for (const s of pair) s.step(STEP);
      acc -= STEP;
    }
    const moving = springs.some((pair) => pair.some((s) => !s.settled()));
    draw();
    dirty = false;
    if (moving && visible) raf = requestAnimationFrame(frame);
    else { last = 0; acc = 0; }
  }

  function kick() {
    if (!raf && visible) raf = requestAnimationFrame(frame);
  }

  // Where the pointer is over the page, 0 to 1 (ds/wallpaper.luau `follow`):
  // each layer of depth d springs to d·step·(1 − 2p), settling back to the
  // middle as the pointer leaves.
  function pose(px, py, instant = false) {
    springs.forEach((pair, i) => {
      const d = i * reach;
      if (d === 0) return;
      pair[0].goal = d * STEP_X + (-2 * d * STEP_X) * px;
      pair[1].goal = d * STEP_Y + (-2 * d * STEP_Y) * py;
      if (instant) for (const s of pair) { s.x = s.goal; s.v = 0; }
    });
    kick();
  }

  function onMove(e) {
    const r = canvas.getBoundingClientRect();
    const k = r.width / WINDOW.w;
    const x = (e.clientX - r.left) / k - PAGE[0], y = (e.clientY - r.top) / k - PAGE[1];
    if (x < 0 || y < 0 || x > PAGE[2] || y > PAGE[3]) return pose(0.5, 0.5);
    pose(Math.min(Math.max(x / PAGE[2], 0), 1), Math.min(Math.max(y / PAGE[3], 0), 1));
  }

  container.appendChild(canvas);
  resize();
  const [px, py] = options.pose ?? [0.5, 0.5];
  pose(px, py, true);
  draw();
  if (!reduced && !options.capture) {
    canvas.addEventListener("pointermove", (e) => { if (e.pointerType === "mouse") onMove(e); }, { passive: true });
    canvas.addEventListener("pointerleave", () => pose(0.5, 0.5), { passive: true });
  }
  // (A size change redraws the chrome once a frame at most.)
  let sizing = 0;
  new ResizeObserver(() => {
    if (sizing) return;
    sizing = requestAnimationFrame(() => {
      sizing = 0;
      resize();
      if (dirty) { draw(); dirty = false; }
    });
  }).observe(container);
  if ("IntersectionObserver" in window) {
    new IntersectionObserver(([entry]) => { visible = entry.isIntersecting; if (visible && (dirty || springs.some((p) => p.some((s) => !s.settled())))) kick(); }).observe(container);
  }
  canvas.addEventListener("webglcontextlost", (e) => { e.preventDefault(); container.dispatchEvent(new CustomEvent("herofailed")); });
  return { canvas, pose, draw, springs };
}
