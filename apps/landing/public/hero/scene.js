// A piece of the Zeron app over one of its parallax scenes, live: the hero
// (the whole new chat) and the gallery's cards (cut-outs) are each one of
// these. A canvas: the chrome in Canvas 2D (drawn once a size), the
// wallpaper and its effect in WebGL2 (wallpaper.js), and a card's moving
// bits (a sprite: the cat's tail and eyes) drawn in GL over it. A frame is
// drawn only while something moves.
//
// mountScene(container, spec, options) → controller, or null without WebGL2
// (the page keeps its still picture then).
//
// spec: {
//   label,                  the canvas' accessible name
//   window: { w, h },       the app window it's laid out in (points)
//   crop: [x, y, w, h],     the part of that window the canvas shows
//   scene: { slug, layers },  the Parallax scenes plugin's scene
//   node, page,             the wallpaper's layer and the page (points)
//   clip, effect            the page card's clip (default the node), and
//                           which ASCII: main's (1) or zeron/zgx's ("ascii2")
//   radius,                 the page card's clip radius
//   pointer: "page" | "crop"  what the pointer's 0..1 is of: the app's page
//                           (the hero: as the app) or the whole card (a
//                           cut-out: its whole width moves the scene as the
//                           page's would)
//   under(ctx, s, opts), over(ctx, s, opts)  the chrome, in window points
//                           (the context translated to the crop)
//   sprites: [{ id, draw(ctx, s), box: [x, y, w, h], origin }]  drawn once
//            or a pixel-art sprite: { id, frames: { name: image }, grid:
//            [w, h], pixel (points a sprite pixel), at: [x, y] } — sampled
//            nearest, a whole number of device pixels a sprite pixel, its
//            corner on the device grid
//   animate(now) → { poses: { [id]: { rotate, alpha, frame } }, busy }  (optional)
//   ready()                 a promise, before the first draw (icons, art)
// }

import { Renderer, texture } from "./wallpaper.js";

// The Parallax scenes plugin's numbers (init.luau): a step of depth moves
// 14 points across and 9 down at the pointer's edge, at Normal; the springs
// are Zeron's (stiffness 140, damping 20).
const STEP_X = 14, STEP_Y = 9;
const SPRING = { stiffness: 140, damping: 20 };
const ART = { w: 576, h: 768 };
// ds/wallpaper.luau: ASCII at a 1.5-point font pixel, Medium strength
// (0.75) and density (no glyph under 0.22).
const ASCII = [1.5, 0.75, 0.22];
// The app steps its springs at its 120 Hz frames.
const STEP = 1 / 120;

// zgx's spring (zgx-script anim.rs): semi-implicit Euler, settling within a
// thousandth.
class Spring {
  constructor(value) { this.x = value; this.goal = value; this.v = 0; }
  settled() { return this.x === this.goal && this.v === 0; }
  step(dt) {
    if (this.settled()) return;
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

export async function mountScene(container, spec, options = {}) {
  const reduced = options.still ?? matchMedia("(prefers-reduced-motion: reduce)").matches;
  const art = options.art ?? "/assets/hero/";
  const [cx, cy, cwPt, chPt] = spec.crop;
  const canvas = document.createElement("canvas");
  canvas.setAttribute("role", "img");
  canvas.setAttribute("aria-label", options.label ?? spec.label ?? "");
  const gl = canvas.getContext("webgl2", { alpha: false, antialias: false, premultipliedAlpha: true, preserveDrawingBuffer: !!options.capture });
  if (!gl) return null;
  const renderer = new Renderer(gl);
  const layers = spec.scene.layers;
  const [images] = await Promise.all([
    Promise.all(layers.map((l) => loadImage(`${art}${spec.scene.slug}/${l}.webp`))),
    spec.ready ? spec.ready() : null,
  ]);
  const pictures = images.map((img) => ({ tex: texture(gl, img), w: ART.w, h: ART.h }));
  const node = spec.node, page = spec.page;
  // The place a plugin draws in: laid at the window's width from the
  // layer's right edge (ds/wallpaper.luau).
  const place = [node[0] + node[2] - spec.window.w, node[1], spec.window.w, node[3]];
  // The reach: Normal (1); Reduce motion holds the scene still (0), as the
  // plugin does. The layers overhang the place by as far as the nearest moves.
  const reach = reduced ? 0 : 1;
  const deepest = layers.length - 1;
  const over = [Math.ceil(deepest * STEP_X * reach), Math.ceil(deepest * STEP_Y * reach)];
  const springs = layers.map(() => [new Spring(0), new Spring(0)]);
  const animated = !reduced && !options.capture && spec.animate;

  let acc = 0, scale = 1, cw = 0, ch = 0, under = null, overTex = null, sprites = [];
  let dirty = true, raf = 0, visible = true, last = 0, poses = {};

  // A canvas of the chrome at the scale, translated to the crop.
  function paint(w, h, draw) {
    const c = document.createElement("canvas");
    c.width = w; c.height = h;
    // (On the CPU: analytic coverage, as the app's; a GPU canvas's 4× MSAA
    // steps edges in quarters.)
    const ctx = c.getContext("2d", { willReadFrequently: true });
    draw(ctx);
    return c;
  }

  function resize() {
    const dpr = options.dpr ?? window.devicePixelRatio ?? 1;
    const css = container.clientWidth || cwPt;
    // (At most the app's own 2x: past it the browser scales the canvas.)
    scale = Math.min((css / cwPt) * dpr, 2);
    const w = Math.round(cwPt * scale), h = Math.round(chPt * scale);
    if (w === cw && h === ch) return;
    cw = w; ch = h;
    canvas.width = w; canvas.height = h;
    const s = scale;
    const opts = { lights: options.lights !== false };
    const translated = (draw) => (ctx) => { ctx.translate(-cx * s, -cy * s); draw(ctx, s, opts); };
    for (const t of [under, overTex, ...sprites.flatMap((sp) => (sp.textures ? Object.values(sp.textures) : [sp.tex]))]) if (t) gl.deleteTexture(t);
    under = texture(gl, paint(w, h, translated(spec.under)));
    overTex = spec.over ? texture(gl, paint(w, h, translated(spec.over))) : null;
    // Each sprite rasterized at the scale on its own box (as zgx paints a
    // turning node into a layer of its own).
    sprites = (spec.sprites || []).map((sp) => {
      if (sp.frames) {
        // (The frames' own pixels, uploaded as they are.)
        const textures = Object.fromEntries(Object.entries(sp.frames).map(([k, img]) => [k, texture(gl, img, { nearest: true })]));
        return { ...sp, textures };
      }
      const [x, y, bw, bh] = sp.box;
      const pw = Math.ceil(bw * s), ph = Math.ceil(bh * s);
      return { ...sp, pw, ph, tex: texture(gl, paint(pw, ph, (ctx) => sp.draw(ctx, s))) };
    });
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
      box: [place[0] - over[0] + springs[i][0].x, place[1] - over[1] + springs[i][1].x, place[2] + 2 * over[0], place[3] + 2 * over[1]],
    }));
    renderer.drawLayer(scale, node, boxes, 0);
    gl.viewport(0, 0, cw, ch);
    renderer.drawFull(under, cw, ch);
    const effect = options.effect === "none" ? 0 : spec.effect || 1;
    renderer.drawEffect(cw, ch, scale, node, ASCII, spec.clip || node, spec.radius, effect, node[3], [cx, cy]);
    if (overTex) renderer.drawFull(overTex, cw, ch);
    for (const sp of sprites) {
      const p = poses[sp.id] || {};
      if (p.alpha === 0) continue;
      if (sp.textures) {
        // A pixel-art sprite: a whole number of device pixels a sprite
        // pixel, its corner on a whole device pixel.
        const k = Math.max(1, Math.round(sp.pixel * scale));
        const x = Math.round((sp.at[0] - cx) * scale), y = Math.round((sp.at[1] - cy) * scale);
        renderer.drawSprite(sp.textures[p.frame || "idle"], cw, ch, x, y, sp.grid[0] * k, sp.grid[1] * k);
        continue;
      }
      const [x, y] = sp.box;
      renderer.drawSprite(sp.tex, cw, ch, (x - cx) * scale, (y - cy) * scale, sp.pw, sp.ph, p.rotate || 0, sp.origin, p.alpha ?? 1);
    }
  }

  function frame(now) {
    raf = 0;
    // Stepped in the app's 1/120 s steps, whatever this display's rate: the
    // layers pass through the app's positions.
    acc += Math.min(last ? (now - last) / 1000 : STEP, 0.25);
    last = now;
    while (acc >= STEP) {
      for (const pair of springs) for (const s of pair) s.step(STEP);
      acc -= STEP;
    }
    let busy = false;
    if (animated) {
      const r = spec.animate(now);
      poses = r.poses;
      busy = r.busy;
    }
    const moving = springs.some((pair) => pair.some((s) => !s.settled()));
    draw();
    dirty = false;
    if ((moving || busy) && visible) raf = requestAnimationFrame(frame);
    else { last = 0; acc = 0; }
  }

  function kick() {
    if (!raf && visible) raf = requestAnimationFrame(frame);
  }

  // Where the pointer is, 0 to 1 (ds/wallpaper.luau `follow`): each layer of
  // depth d springs to d·step·(1 − 2p), settling back as the pointer leaves.
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
    const k = r.width / cwPt;
    const wx = (e.clientX - r.left) / k + cx, wy = (e.clientY - r.top) / k + cy;
    const box = spec.pointer === "crop" ? spec.crop : page;
    const x = wx - box[0], y = wy - box[1];
    if (x < 0 || y < 0 || x > box[2] || y > box[3]) return pose(0.5, 0.5);
    pose(Math.min(Math.max(x / box[2], 0), 1), Math.min(Math.max(y / box[3], 0), 1));
  }

  container.appendChild(canvas);
  resize();
  const [px, py] = options.pose ?? [0.5, 0.5];
  pose(px, py, true);
  if (animated) poses = spec.animate(performance.now()).poses;
  draw();
  if (!reduced && !options.capture) {
    canvas.addEventListener("pointermove", (e) => { if (e.pointerType === "mouse") onMove(e); }, { passive: true });
    canvas.addEventListener("pointerleave", () => pose(0.5, 0.5), { passive: true });
  }
  // A card's own life (a blink, a flick) asks for frames when it starts.
  if (animated && spec.onWake) spec.onWake(kick);
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
    new IntersectionObserver(([entry]) => {
      visible = entry.isIntersecting;
      if (visible && (dirty || springs.some((p) => p.some((s) => !s.settled())))) kick();
    }).observe(container);
  }
  canvas.addEventListener("webglcontextlost", (e) => { e.preventDefault(); container.dispatchEvent(new CustomEvent("herofailed")); });
  return { canvas, pose, draw, springs };
}
