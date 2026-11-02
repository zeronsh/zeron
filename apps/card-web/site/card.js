// Zeron's member card for the web: a port of apps/zgx/ui/ds/member_card.luau
// (the native card) to WebGL2, drawing the same picture from the same
// numbers. The card is 300 by 420 points: layered art, the sky in the
// intro's characters, paper rising from below under the member's name,
// badges and the day they joined, tilting toward the pointer, its foil (one
// of five finishes) catching the light. Text is set by the browser (Canvas
// 2D) into a texture; everything else is the native shaders' maths.
//
//   mountCard(container, card, options) -> controller
//
// `card`: the public card the edge serves (`username`, `firstName`,
// `userNumber`, `joinedOn`, `badges`, `scene`, `holo`, `sky`) with its `art`
// resolved from the scenes manifest (`artFor`), and `tagline` if there is
// one. Options: `theme` (dark | light | auto), `caption` (the print's caption
// under the card, off by default: a member's page shows only what the public
// read returns), `arrive`, `still`
// (true, false or "auto" for the system's Reduce motion), `scale` ("fit" or
// a number), `origin` ({x, y} in CSS px, or null to centre), `dpr`, and
// `rig` (a frozen pose for captures: {pose: [x, y] | null, lit, strength,
// drift: [d1, d2, d3] | ms, t}). No dependencies, no framework.

export const W = 300;
export const H = 420;
const BORDER = 1;
const RADIUS = 16;
const TILT = 12;
const PAD = 16;
const MARGIN = 2; // the card texture's resampling edge, in points
export const HOLOS = ["aurora", "reverse", "secret", "starlight", "glyph"];

// `zeron.card.*` tokens: the same in either theme (a printed object).
const TOKENS = {
  paper: [0xf1, 0xef, 0xe8],
  ink: "#161616",
  muted: "#55534e",
  light: "#ffffff",
  pill: [0x16, 0x16, 0x16, 0x80],
  rule: [0x16, 0x16, 0x16, 0x33],
  edge: [0, 0, 0, 0x1f],
  shadow: { x: 0, y: 24, sigma: 24, color: [0, 0, 0, 0x4d] },
  size: { name: 34, tagline: 12, number: 12, mark: 16, caption: 11, badge: 10 }
};
// What the themes give: the window behind the card and the caption's ink.
export const THEMES = {
  dark: { window: "#0e0e10", caption: "rgba(237,237,239,0.42)" },
  light: { window: "#f6f6f7", caption: "rgba(27,27,29,0.45)" }
};
// The paper: a vertical gradient, stops as the native card's.
const PAPER_STOPS = [[0.28, 0], [0.38, 0x1a], [0.46, 0x4d], [0.54, 0x99], [0.61, 0xd1], [0.68, 0xf0], [0.76, 0xff]];
// A foil badge's gradient (120 degrees).
const FOIL_STOPS = [[0xc9, 0xd6, 0xff, 0xcc], [0xef, 0xd0, 0xf2, 0xcc], [0xff, 0xe0, 0xc6, 0xcc], [0xcd, 0xf1, 0xe6, 0xcc]];

// How the public read's badges map onto the card's pills. The read gives an
// id, a name and a free-form icon; the card wants a style (ink, outline or
// foil) and, if any, one of the intro's characters. Cloud Tester is an
// outline pill with no glyph, as the native sign-in wall shows it; a badge
// this table doesn't know is an ink pill, the native card's default.
export const BADGE_STYLES = { "cloud-tester": "outline" };
export const BADGE_GLYPHS = { hash: "#", at: "@", sparkle: "+", star: "x", plus: "+", dot: ".", dash: "-" };

// `since` as the card prints it: "Oct 4, 2023" from "2023-10-04" (UTC).
export function sinceOf(joinedOn) {
  const d = new Date(`${joinedOn}T00:00:00Z`);
  if (Number.isNaN(d.getTime())) return String(joinedOn);
  return d.toLocaleDateString("en-US", { month: "short", day: "numeric", year: "numeric", timeZone: "UTC" });
}

// The art for `scene` (0-based, into the manifest ordered by `number`), its
// files under `base`.
export function artFor(manifest, scene, base = "/card/art") {
  const scenes = [...manifest].sort((a, b) => a.number - b.number);
  const s = scenes[((scene % scenes.length) + scenes.length) % scenes.length];
  return {
    slug: s.slug,
    number: s.number,
    title: s.title,
    finish: s.finish,
    layers: s.layers.map((l) => ({ src: `${base}/${l.file}`, depth: l.depth }))
  };
}

// ---------------------------------------------------------------- motion

// zgx's spring (zgx-script anim.rs): semi-implicit Euler at the frame's
// interval, capped at a 30th of a second, settling within a thousandth.
class Spring {
  constructor(value, stiffness, damping) {
    this.x = value; this.goal = value; this.v = 0; this.k = stiffness; this.c = damping;
  }
  settled() { return this.x === this.goal; }
  step(dt) {
    if (this.x === this.goal) return;
    dt = Math.min(dt, 1 / 30);
    const to = this.goal;
    this.v += (-this.k * (this.x - to) - this.c * this.v) * dt;
    const x = this.x + this.v * dt;
    const settle = 1e-3 * Math.max(Math.abs(to), 1);
    if (!Number.isFinite(x) || (Math.abs(x - to) < settle && Math.abs(this.v) < settle * 10)) {
      this.x = to; this.v = 0;
    } else {
      this.x = x;
    }
  }
}
// A tween toward a goal over `ms` with an ease; retargeting restarts from
// where it is.
class Tween {
  constructor(value, ms, ease) { this.x = value; this.goal = value; this.ms = ms; this.ease = ease; this.t0 = null; this.from = value; }
  set(goal, now) { if (goal === this.goal) return; this.goal = goal; this.from = this.x; this.t0 = now; }
  settled() { return this.x === this.goal; }
  step(now) {
    if (this.x === this.goal) return;
    const t = (now - this.t0) / this.ms;
    this.x = t >= 1 ? this.goal : this.from + (this.goal - this.from) * this.ease(t);
  }
}
const outQuad = (t) => 1 - (1 - t) * (1 - t);
const outCubic = (t) => 1 - (1 - t) ** 3;
const inOutSine = (t) => -(Math.cos(Math.PI * t) - 1) / 2;
const lerp = (a, b, t) => a + (b - a) * t;

// ---------------------------------------------------------------- shaders

const GLYPHS = `
uvec2 glyph(uint i) {
  switch (i) {
    case 1u: return uvec2(402653184u, 12u); case 2u: return uvec2(1015808u, 0u);
    case 3u: return uvec2(139432064u, 0u); case 4u: return uvec2(340083712u, 17u);
    case 5u: return uvec2(581052977u, 17u); case 6u: return uvec2(35078671u, 30u);
    case 7u: return uvec2(588727854u, 14u); case 8u: return uvec2(597347886u, 14u);
    case 9u: return uvec2(368409930u, 10u); case 10u: return uvec2(561700398u, 15u);
    default: return uvec2(0u);
  }
}
float cell_hash(uvec2 c) {
  uint h = c.x * 1664525u + c.y * 1013904223u + 0x9e3779b9u;
  h ^= h >> 16u; h *= 2246822519u; h ^= h >> 13u; h *= 3266489917u; h ^= h >> 16u;
  return float(h & 16777215u) / 16777216.0;
}
float glyph_lit(uvec2 g, uvec2 fp) {
  if (fp.x < 5u && fp.y < 7u) {
    uint row = fp.y < 6u ? (g.x >> (fp.y * 5u)) : g.y;
    return float((row >> (4u - fp.x)) & 1u);
  }
  return 0.0;
}`;

const PRELUDE = `#version 300 es
precision highp float; precision highp int;
uniform vec2 u_size; uniform float u_s; uniform float u_flip; uniform vec2 u_origin;
uniform vec4 u_clip; uniform float u_clipR; uniform float u_opacity;
in vec2 v_uv; out vec4 o;
vec2 pdev() { return vec2(gl_FragCoord.x, u_flip > 0.5 ? u_size.y - gl_FragCoord.y : gl_FragCoord.y); }
vec2 ppt() { return pdev() / u_s - u_origin; }
float rrect(vec2 q, vec2 hw, float r) { vec2 d = abs(q) - hw + vec2(r); return length(max(d, vec2(0.0))) + min(max(d.x, d.y), 0.0) - r; }
// zgx's rounded clip region: cut a device pixel wide at the box's edge.
float clipK(vec2 p) {
  if (u_clip.z < 0.0) return 1.0;
  vec2 hw = u_clip.zw * 0.5;
  return clamp(0.5 - rrect(p - u_clip.xy - hw, hw, u_clipR) * u_s, 0.0, 1.0);
}
// zgx's \`centred\`: how far the pixel's centre is from the box's middle (on a
// 64th of a device pixel), each way, in points.
vec2 centred(vec2 at, vec4 box) {
  vec2 middle = round((box.xy + u_origin + box.zw * 0.5) * u_s * 64.0) / 64.0;
  return abs(at - middle) / u_s;
}
`;

const VERT = `#version 300 es
precision highp float;
in vec2 a_pos; in vec2 a_uv; out vec2 v_uv;
uniform vec2 u_size; uniform float u_s; uniform float u_flip; uniform vec2 u_origin; uniform mat3 u_m;
void main() {
  vec3 q = u_m * vec3(a_pos, 1.0);
  float w = max(q.z, 1e-4);
  vec2 t = (q.xy + u_origin * w) * u_s;
  float x = t.x / u_size.x * 2.0 - w;
  float y = u_flip > 0.5 ? w - t.y / u_size.y * 2.0 : t.y / u_size.y * 2.0 - w;
  gl_Position = vec4(x, y, 0.0, w);
  v_uv = a_uv;
}`;

// A premultiplied texture over the box, through the clip.
const FRAG_IMAGE = PRELUDE + `
uniform sampler2D u_tex;
void main() { o = texture(u_tex, v_uv) * u_opacity * clipK(ppt()); }`;

// The sky in characters (member_card.luau's \`sky\` layer shader): each cell
// a glyph picked by the picture's brightness there, lit in its colour, over
// the picture a little dimmed.
const FRAG_SKY = PRELUDE + GLYPHS + `
uniform sampler2D u_tex; uniform vec2 u_node; uniform float u_px; uniform vec2 u_grid;
vec4 sampleSky(vec2 p) { vec4 t = texture(u_tex, p / u_node); return t.a > 0.0 ? vec4(t.rgb / t.a, t.a) : vec4(0.0); }
void main() {
  vec2 pc = ppt();
  vec2 p0 = pc - vec2(1.0);
  if (any(lessThan(p0, vec2(0.0))) || any(greaterThanEqual(p0, u_node))) { o = vec4(0.0); return; }
  float px = max(u_px, 1.0);
  vec2 cell_size = vec2(6.0, 8.0) * px;
  // The field's grid starts \`u_grid\` points in (the seed's sky, within one cell).
  vec2 p = p0 + u_grid;
  vec2 cell = floor(p / cell_size);
  vec4 at = sampleSky((cell + 0.5) * cell_size - u_grid);
  vec4 own = sampleSky(p0);
  uint index = uint(pow(clamp((dot(at.rgb, vec3(0.299, 0.587, 0.114)) * at.a - 0.3) / 0.65, 0.0, 1.0), 1.6) * 10.999);
  uvec2 g = glyph(index);
  uvec2 fp = uvec2(floor((p - cell * cell_size) / px));
  float lit = glyph_lit(g, fp);
  vec3 ink = at.rgb / max(max(at.r, max(at.g, at.b)), 0.25);
  float k = lit * 0.62;
  vec4 c = clamp(vec4(ink * k + own.rgb * 0.88 * (1.0 - k), own.a), 0.0, 1.0);
  o = vec4(c.rgb * c.a, c.a) * u_opacity * clipK(pc);
}`;

// The paper: a vertical gradient over the node, premultiplied.
const FRAG_PAPER = PRELUDE + `
uniform vec2 u_node; uniform vec3 u_paper; uniform float u_stops[14];
void main() {
  vec2 pc = ppt(); vec2 p = pc - vec2(1.0);
  float t = p.y / u_node.y;
  float a = u_stops[1];
  for (int i = 0; i < 6; i++) {
    float t0 = u_stops[i * 2], t1 = u_stops[i * 2 + 2];
    if (t >= t0 && t <= t1) { a = mix(u_stops[i * 2 + 1], u_stops[i * 2 + 3], (t - t0) / max(t1 - t0, 1e-6)); }
  }
  if (t > u_stops[12]) a = u_stops[13];
  o = vec4(u_paper * a, a) * u_opacity * clipK(pc);
}`;

// A box: fill and border with rounded corners, antialiased as zgx's quad
// shader does it (\`shade\`'s fill branch), optionally a gradient fill.
const FRAG_BOX = PRELUDE + `
uniform vec4 u_box; uniform float u_radius; uniform float u_border;
uniform vec4 u_fill; uniform vec4 u_borderColor;
uniform float u_grad; uniform vec4 u_gstops[4]; uniform float u_gangle;
vec4 gradientAt(vec2 p) {
  float a = radians(u_gangle);
  vec2 dir = vec2(sin(a), -cos(a));
  float len = abs(u_box.z * dir.x) + abs(u_box.w * dir.y);
  float t = clamp(dot(p - (u_box.xy + u_box.zw * 0.5), dir) / len + 0.5, 0.0, 1.0);
  float f = t * 3.0; int i = int(min(floor(f), 2.0)); float k = f - float(i);
  vec4 c0 = u_gstops[i], c1 = u_gstops[i + 1];
  vec4 p0 = vec4(c0.rgb * c0.a, c0.a), p1 = vec4(c1.rgb * c1.a, c1.a);
  return mix(p0, p1, k);
}
void main() {
  vec2 pc = ppt();
  float radius = clamp(u_radius, 0.0, min(u_box.z, u_box.w) * 0.5);
  vec2 q = centred(pdev(), u_box) - (u_box.zw * 0.5 - vec2(radius));
  float dist = length(max(q, vec2(0.0))) + min(max(q.x, q.y), 0.0) - radius;
  vec2 outside = max(q, vec2(0.0)); float reach = length(outside);
  vec2 gradient = reach > 0.0 ? outside / max(reach, 1e-20) : (q.x > q.y ? vec2(1.0, 0.0) : vec2(0.0, 1.0));
  vec2 slope = gradient / u_s;
  float aa = max(radius <= 0.0 ? max(slope.x, slope.y) : slope.x + slope.y, 0.5);
  float outer = clamp(0.5 - dist / aa, 0.0, 1.0);
  float inner = clamp(0.5 - (dist + u_border) / aa, 0.0, 1.0);
  vec4 fill = u_grad > 0.5 ? gradientAt(pc) : vec4(u_fill.rgb * u_fill.a, u_fill.a);
  vec4 border = vec4(u_borderColor.rgb * u_borderColor.a, u_borderColor.a);
  o = (fill * inner + border * (outer - inner)) * u_opacity * clipK(pc);
}`;

// The card's own border: a ring (zgx's \`ring_quad\`), outer corners at the
// card's radius, inner ones less the border.
const FRAG_RING = PRELUDE + `
uniform vec4 u_box; uniform float u_radius; uniform float u_width; uniform vec4 u_color;
void main() {
  vec2 at = pdev();
  float radius = clamp(u_radius, 0.0, min(u_box.z, u_box.w) * 0.5);
  vec2 q = centred(at, u_box) - (u_box.zw * 0.5 - vec2(radius));
  float dist = length(max(q, vec2(0.0))) + min(max(q.x, q.y), 0.0) - radius;
  vec2 outside = max(q, vec2(0.0)); float reach = length(outside);
  vec2 gradient = reach > 0.0 ? outside / max(reach, 1e-20) : (q.x > q.y ? vec2(1.0, 0.0) : vec2(0.0, 1.0));
  vec2 slope = gradient / u_s;
  vec4 box = vec4(u_box.xy + vec2(u_width), max(u_box.zw - vec2(2.0 * u_width), vec2(0.0)));
  float inner_radius = clamp(radius - u_width, 0.0, min(box.z, box.w) * 0.5);
  vec2 iq = centred(at, box) - (box.zw * 0.5 - vec2(inner_radius));
  float inside = length(max(iq, vec2(0.0))) + min(max(iq.x, iq.y), 0.0) - inner_radius;
  float aa = max(slope.x + slope.y, 0.5);
  float hollow = (box.z > 0.0 && box.w > 0.0) ? clamp(0.5 - inside / aa, 0.0, 1.0) : 0.0;
  float k = max(clamp(0.5 - dist / aa, 0.0, 1.0) - hollow, 0.0) * u_color.a * u_opacity;
  o = vec4(u_color.rgb * k, k);
}`;

// The box shadow: the box blurred (zgx's shadow branch: erfc of the signed
// distance over sigma, hw covered at the edge).
const FRAG_SHADOW = PRELUDE + `
uniform vec4 u_box; uniform float u_radius; uniform float u_sigma; uniform vec4 u_color;
void main() {
  float radius = clamp(u_radius, 0.0, min(u_box.z, u_box.w) * 0.5);
  vec2 q = centred(pdev(), u_box) - (u_box.zw * 0.5 - vec2(radius));
  float dist = length(max(q, vec2(0.0))) + min(max(q.x, q.y), 0.0) - radius;
  float x = dist / (u_sigma * 1.41421356);
  float x2 = x * x;
  float e = sign(x) * sqrt(1.0 - exp(-x2 * (1.27323954 + 0.147 * x2) / (1.0 + 0.147 * x2)));
  float k = (0.5 - 0.5 * e) * u_color.a * u_opacity;
  o = vec4(u_color.rgb * k, k);
}`;

// The foil: the shine (one of five finishes) under the glare, each at its
// own opacity, over the node (member_card.luau's \`shine\` and \`glare\`).
const FRAG_FOIL = PRELUDE + GLYPHS + `
uniform vec2 u_node; uniform vec2 u_at; uniform float u_k; uniform int u_finish;
vec3 spectrum(float t) { return vec3(0.5) + 0.5 * cos(6.2831853 * (vec3(fract(t)) + vec3(0.0, 0.33, 0.67))); }
float hash2(vec2 c) { return fract(sin(dot(c, vec2(127.1, 311.7))) * 43758.5453); }
vec4 shine(vec2 p) {
  vec2 uv = p / u_node;
  vec2 at = u_at; float k = u_k;
  if (k <= 0.001) return vec4(0.0);
  vec2 tilt = at - vec2(0.5);
  float paper = smoothstep(0.5, 0.62, uv.y);
  float quiet = mix(1.0, 0.4, paper);
  float across = (uv.x - at.x) + (uv.y - at.y);
  vec3 rgb = vec3(1.0); float a = 0.0;
  if (u_finish == 1) {
    vec2 cell = floor(p / 2.5);
    float h = hash2(cell);
    float glint = pow(max(sin(h * 40.0 + tilt.x * 9.0 + tilt.y * 7.0), 0.0), 14.0) * step(0.5, h);
    rgb = mix(spectrum(h * 2.0 + tilt.x), vec3(1.0), 0.35);
    a = (glint * 0.7 + 0.05) * mix(1.0, 0.35, paper);
  } else if (u_finish == 2) {
    float d1 = fract((p.x + p.y) / 4.0);
    float d2 = fract((p.x - p.y) / 4.0);
    float lines = max(1.0 - smoothstep(0.0, 0.2, abs(d1 - 0.5)), 1.0 - smoothstep(0.0, 0.2, abs(d2 - 0.5)));
    rgb = spectrum((uv.x - uv.y) * 1.3 + tilt.x * 1.6 - tilt.y);
    a = lines * (0.1 + 0.26 * exp(-across * across * 3.0)) * quiet;
  } else if (u_finish == 3) {
    float size = 16.0;
    vec2 cell = floor(p / size);
    float h = hash2(cell);
    vec2 centre = (cell + vec2(0.3 + 0.4 * hash2(cell + 7.0), 0.3 + 0.4 * hash2(cell + 13.0))) * size;
    vec2 d = abs(p - centre);
    float star = exp(-d.x * 0.7 - d.y * 4.0) + exp(-d.y * 0.7 - d.x * 4.0) + exp(-dot(d, d) * 0.5);
    float glint = pow(max(sin(h * 30.0 + tilt.x * 8.0 - tilt.y * 6.0), 0.0), 6.0) * step(0.62, h);
    rgb = mix(vec3(1.0), spectrum(h + uv.y), 0.3);
    a = (min(star * glint, 1.0) * 0.95 + 0.05 * exp(-across * across * 4.0)) * quiet;
  } else if (u_finish == 4) {
    float px = 1.5;
    vec2 cell_size = vec2(6.0, 8.0) * px;
    vec2 cell = floor(p / cell_size);
    uvec2 c = uvec2(max(cell, vec2(0.0)));
    uvec2 g = glyph(1u + uint(cell_hash(c) * 9.999));
    uvec2 fp = uvec2(floor((p - cell * cell_size) / px));
    float lit = glyph_lit(g, fp);
    rgb = spectrum(uv.y * 1.5 + uv.x * 0.7 + tilt.x);
    a = lit * exp(-across * across * 7.0) * 0.6 * mix(1.0, 0.3, paper);
  } else {
    float x = uv.x * 2.2 - tilt.x * 2.0 + uv.y * 0.35;
    float pillar = smoothstep(0.0, 1.0, 0.5 + 0.5 * cos(x * 6.2831853));
    rgb = mix(spectrum(uv.x * 0.6 - tilt.x * 1.1 + uv.y * 0.25), vec3(1.0), 0.35);
    float along = smoothstep(0.0, 0.25, uv.y) * (1.0 - smoothstep(0.75, 1.0, uv.y));
    a = pillar * 0.24 * along * quiet;
  }
  return vec4(rgb, a * k);
}
vec4 glare(vec2 p) {
  vec2 uv = p / u_node;
  vec2 off = uv - u_at;
  if (u_k <= 0.001) return vec4(0.0);
  return vec4(vec3(1.0), exp(-dot(off, off) * 5.0) * 0.22 * u_k);
}
void main() {
  vec2 pc = ppt(); vec2 p = pc - vec2(1.0);
  vec4 s = clamp(shine(p), 0.0, 1.0); vec4 g = clamp(glare(p), 0.0, 1.0);
  float sa = s.a * u_opacity, ga = g.a * u_opacity;
  vec4 sp = vec4(s.rgb * sa, sa), gp = vec4(g.rgb * ga, ga);
  o = (gp + sp * (1.0 - gp.a)) * clipK(pc);
}`;

// ---------------------------------------------------------------- GL plumbing

function compile(gl, type, src) {
  const sh = gl.createShader(type);
  gl.shaderSource(sh, src);
  gl.compileShader(sh);
  if (!gl.getShaderParameter(sh, gl.COMPILE_STATUS)) {
    throw new Error(`shader: ${gl.getShaderInfoLog(sh)}`);
  }
  return sh;
}
function program(gl, frag) {
  const p = gl.createProgram();
  gl.attachShader(p, compile(gl, gl.VERTEX_SHADER, VERT));
  gl.attachShader(p, compile(gl, gl.FRAGMENT_SHADER, frag));
  gl.bindAttribLocation(p, 0, "a_pos");
  gl.bindAttribLocation(p, 1, "a_uv");
  gl.linkProgram(p);
  if (!gl.getProgramParameter(p, gl.LINK_STATUS)) throw new Error(`program: ${gl.getProgramInfoLog(p)}`);
  const uniforms = {};
  const n = gl.getProgramParameter(p, gl.ACTIVE_UNIFORMS);
  for (let i = 0; i < n; i++) {
    const info = gl.getActiveUniform(p, i);
    const name = info.name.replace(/\[0\]$/, "");
    uniforms[name] = gl.getUniformLocation(p, info.name);
  }
  return { p, u: uniforms };
}
const IDENTITY = new Float32Array([1, 0, 0, 0, 1, 0, 0, 0, 1]);

class Gfx {
  constructor(canvas) {
    const gl = canvas.getContext("webgl2", { alpha: true, premultipliedAlpha: true, antialias: false, preserveDrawingBuffer: false, powerPreference: "low-power" });
    if (!gl) throw new Error("WebGL2 is not available");
    this.gl = gl;
    this.canvas = canvas;
    gl.enable(gl.BLEND);
    gl.blendFunc(gl.ONE, gl.ONE_MINUS_SRC_ALPHA);
    gl.pixelStorei(gl.UNPACK_COLORSPACE_CONVERSION_WEBGL, gl.NONE);
    this.programs = {
      image: program(gl, FRAG_IMAGE), sky: program(gl, FRAG_SKY), paper: program(gl, FRAG_PAPER),
      box: program(gl, FRAG_BOX), ring: program(gl, FRAG_RING), shadow: program(gl, FRAG_SHADOW), foil: program(gl, FRAG_FOIL)
    };
    this.vao = gl.createVertexArray();
    gl.bindVertexArray(this.vao);
    this.vbo = gl.createBuffer();
    gl.bindBuffer(gl.ARRAY_BUFFER, this.vbo);
    gl.bufferData(gl.ARRAY_BUFFER, 16 * 4, gl.DYNAMIC_DRAW);
    gl.enableVertexAttribArray(0);
    gl.vertexAttribPointer(0, 2, gl.FLOAT, false, 16, 0);
    gl.enableVertexAttribArray(1);
    gl.vertexAttribPointer(1, 2, gl.FLOAT, false, 16, 8);
    this.quad = new Float32Array(16);
  }
  texture(width, height) {
    const gl = this.gl;
    const t = gl.createTexture();
    gl.bindTexture(gl.TEXTURE_2D, t);
    gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_MIN_FILTER, gl.LINEAR);
    gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_MAG_FILTER, gl.LINEAR);
    gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_WRAP_S, gl.CLAMP_TO_EDGE);
    gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_WRAP_T, gl.CLAMP_TO_EDGE);
    if (width) gl.texImage2D(gl.TEXTURE_2D, 0, gl.RGBA, width, height, 0, gl.RGBA, gl.UNSIGNED_BYTE, null);
    return t;
  }
  // A premultiplied texture from an image or canvas (row 0 at v = 0, the top).
  upload(source) {
    const gl = this.gl;
    const t = this.texture();
    gl.pixelStorei(gl.UNPACK_PREMULTIPLY_ALPHA_WEBGL, true);
    gl.pixelStorei(gl.UNPACK_FLIP_Y_WEBGL, false);
    gl.texImage2D(gl.TEXTURE_2D, 0, gl.RGBA, gl.RGBA, gl.UNSIGNED_BYTE, source);
    gl.pixelStorei(gl.UNPACK_PREMULTIPLY_ALPHA_WEBGL, false);
    return t;
  }
  // An offscreen target `w` by `h` points at `s` device pixels a point, its
  // content origin `origin` points in.
  target(wpt, hpt, s, origin) {
    const gl = this.gl;
    const width = Math.max(1, Math.round(wpt * s)), height = Math.max(1, Math.round(hpt * s));
    const tex = this.texture(width, height);
    const fbo = gl.createFramebuffer();
    gl.bindFramebuffer(gl.FRAMEBUFFER, fbo);
    gl.framebufferTexture2D(gl.FRAMEBUFFER, gl.COLOR_ATTACHMENT0, gl.TEXTURE_2D, tex, 0);
    gl.bindFramebuffer(gl.FRAMEBUFFER, null);
    return { fbo, tex, width, height, s, origin, flip: 0, wpt, hpt };
  }
  bind(target) {
    const gl = this.gl;
    gl.bindFramebuffer(gl.FRAMEBUFFER, target.fbo);
    gl.viewport(0, 0, target.width, target.height);
    this.current = target;
  }
  clear() {
    const gl = this.gl;
    gl.clearColor(0, 0, 0, 0);
    gl.clear(gl.COLOR_BUFFER_BIT);
  }
  // Draws the quad `x, y, w, h` (points in the current target's content space)
  // with program `name`, `set` filling its own uniforms. `clip`: a rounded
  // box {x, y, w, h, r} or null; `m`: a projective 3x3 (column-major) or null.
  draw(name, x, y, w, h, set, { clip = null, opacity = 1, m = null, uv = [0, 0, 1, 1] } = {}) {
    const gl = this.gl, t = this.current, pr = this.programs[name];
    gl.useProgram(pr.p);
    gl.uniform2f(pr.u.u_size, t.width, t.height);
    gl.uniform1f(pr.u.u_s, t.s);
    gl.uniform1f(pr.u.u_flip, t.flip);
    gl.uniform2f(pr.u.u_origin, t.origin[0], t.origin[1]);
    if (pr.u.u_clip) {
      if (clip) { gl.uniform4f(pr.u.u_clip, clip.x, clip.y, clip.w, clip.h); gl.uniform1f(pr.u.u_clipR, clip.r); }
      else { gl.uniform4f(pr.u.u_clip, 0, 0, -1, -1); gl.uniform1f(pr.u.u_clipR, 0); }
    }
    if (pr.u.u_opacity) gl.uniform1f(pr.u.u_opacity, opacity);
    gl.uniformMatrix3fv(pr.u.u_m, false, m || IDENTITY);
    set(pr.u, gl);
    const q = this.quad;
    const [u0, v0, u1, v1] = uv;
    q.set([x, y, u0, v0, x + w, y, u1, v0, x, y + h, u0, v1, x + w, y + h, u1, v1]);
    gl.bindVertexArray(this.vao);
    gl.bindBuffer(gl.ARRAY_BUFFER, this.vbo);
    gl.bufferSubData(gl.ARRAY_BUFFER, 0, q);
    gl.drawArrays(gl.TRIANGLE_STRIP, 0, 4);
  }
  image(tex, x, y, w, h, opts) {
    const gl = this.gl;
    this.draw("image", x, y, w, h, (u) => { gl.activeTexture(gl.TEXTURE0); gl.bindTexture(gl.TEXTURE_2D, tex); gl.uniform1i(u.u_tex, 0); }, opts);
  }
}

// ---------------------------------------------------------------- text

// The card's type: Geist's ascent and descent (1.005 and 0.295 em), its
// lines zgx's normal pitch (1.4 em, rounded up) unless given.
const ASCENT = 1.005, DESCENT = 0.295;
const lineOf = (size) => Math.ceil(size * 1.4);
const baselineIn = (top, line, size) => top + (line - (ASCENT + DESCENT) * size) / 2 + ASCENT * size;
// Geist Mono's advance, 0.6 em; letter spacing follows every glyph, the
// last one too, as zgx's shaper adds it.
const monoWidth = (text, size, spacing) => [...text].length * (0.6 * size + spacing);

const FONT_SANS = "Geist", FONT_MONO = "Geist Mono";
const supportsLetterSpacing = typeof CanvasRenderingContext2D !== "undefined" && "letterSpacing" in CanvasRenderingContext2D.prototype;

// Draws a text run at `x`, `baseline` (points) into `ctx` at `s` pixels a
// point. Mono runs place each glyph on its own advance; sans runs let the
// browser shape the whole string with its spacing.
function fillRun(ctx, s, run) {
  const { text, x, baseline, size, family, weight, color, spacing = 0, mono = false } = run;
  ctx.font = `${weight} ${size * s}px "${family}"`;
  ctx.fillStyle = color;
  ctx.textBaseline = "alphabetic";
  ctx.textAlign = "left";
  if (mono) {
    const chars = [...text];
    const advance = 0.6 * size + spacing;
    chars.forEach((ch, i) => ctx.fillText(ch, (x + i * advance) * s, baseline * s));
  } else if (spacing === 0) {
    ctx.fillText(text, x * s, baseline * s);
  } else if (supportsLetterSpacing) {
    ctx.letterSpacing = `${spacing * s}px`;
    ctx.fillText(text, x * s, baseline * s);
    ctx.letterSpacing = "0px";
  } else {
    let cx = x;
    for (const ch of [...text]) {
      ctx.fillText(ch, cx * s, baseline * s);
      cx += ctx.measureText(ch).width / s + spacing;
    }
  }
}

// ---------------------------------------------------------------- the card

// Where everything sits on the card, in points from its top-left corner
// (the native card's layout: a 1-point border, 16 of padding, pills 22
// high, badges 20, Geist Mono at 0.6 em).
// "004·182": six digits, a centred dot between their halves (the native
// card's `numbered`).
export function serial(n) {
  const v = Math.max(0, Math.trunc(Number(n) || 0));
  const pad = (x) => String(x % 1000).padStart(3, "0");
  return `${pad(Math.trunc(v / 1000))}·${pad(v)}`;
}

export function layoutCard(card) {
  const name = card.firstName || `@${card.username}`;
  const since = sinceOf(card.joinedOn);
  const number = serial(card.userNumber);
  const x0 = BORDER + PAD, x1 = W - BORDER - PAD, y0 = BORDER + PAD, y1 = H - BORDER - PAD;
  const runs = [], boxes = [];
  const sz = TOKENS.size;
  // The mark and the number, on the sky.
  const markLine = lineOf(sz.mark);
  runs.push({ text: "zeron", x: x0, baseline: baselineIn(y0, markLine, sz.mark), size: sz.mark, family: FONT_SANS, weight: 700, color: TOKENS.light });
  const rowH = Math.max(markLine, 22);
  const numW = PAD + monoWidth(number, sz.number, 1);
  const numBox = { x: x1 - numW, y: y0 + (rowH - 22) / 2, w: numW, h: 22, r: 9999, fill: TOKENS.pill };
  boxes.push(numBox);
  runs.push({ text: number, x: numBox.x + 8, baseline: baselineIn(numBox.y + (22 - lineOf(sz.number)) / 2, lineOf(sz.number), sz.number), size: sz.number, family: FONT_MONO, weight: 400, color: TOKENS.light, spacing: 1, mono: true });
  // From the bottom up: the day they joined, the tagline, the name, the badges.
  const sinceW = PAD + monoWidth(since, sz.number, 1);
  const sinceBox = { x: x0, y: y1 - 22, w: sinceW, h: 22, r: 9999, fill: TOKENS.pill };
  boxes.push(sinceBox);
  runs.push({ text: since, x: sinceBox.x + 8, baseline: baselineIn(sinceBox.y + (22 - lineOf(sz.number)) / 2, lineOf(sz.number), sz.number), size: sz.number, family: FONT_MONO, weight: 400, color: TOKENS.light, spacing: 1, mono: true });
  let top = sinceBox.y - 12;
  if (card.tagline) {
    const line = lineOf(sz.tagline);
    top -= line;
    runs.push({ text: String(card.tagline).toUpperCase(), x: x0, baseline: baselineIn(top, line, sz.tagline), size: sz.tagline, family: FONT_MONO, weight: 400, color: TOKENS.muted, spacing: 3, mono: true });
    top -= 4;
  }
  const nameLine = lineOf(sz.name);
  top -= nameLine;
  runs.push({ text: name, x: x0, baseline: baselineIn(top, nameLine, sz.name), size: sz.name, family: FONT_SANS, weight: 600, color: TOKENS.ink, spacing: -1 });
  const badges = (card.badges || []).map(badgeOf);
  if (badges.length) {
    // Wrapped into rows 20 high, 4 apart, as the flex row wraps them.
    const rows = [[]];
    let rowW = 0;
    for (const b of badges) {
      const w = badgeWidth(b);
      if (rowW > 0 && rowW + 4 + w > x1 - x0) { rows.push([]); rowW = 0; }
      if (rowW > 0) rowW += 4;
      rows[rows.length - 1].push({ ...b, w, x: x0 + rowW });
      rowW += w;
    }
    const rowsH = rows.length * 20 + (rows.length - 1) * 4;
    top -= 12 + rowsH;
    rows.forEach((row, i) => {
      const y = top + i * 24;
      for (const b of row) {
        const outline = b.style === "outline";
        const box = { x: b.x, y, w: b.w, h: 20, r: 9999 };
        if (b.style === "ink") box.fill = TOKENS.pill;
        if (outline) { box.border = 1; box.borderColor = TOKENS.rule; }
        if (b.style === "foil") box.gradient = { angle: 120, stops: FOIL_STOPS };
        boxes.push(box);
        const ink = b.style === "ink" ? TOKENS.light : TOKENS.ink;
        let cx = b.x + 8 + (outline ? 1 : 0);
        const lineTop = y + 4;
        const baseline = baselineIn(lineTop, 12, sz.badge);
        if (b.glyph) {
          runs.push({ text: b.glyph, x: cx + (10 - 0.6 * sz.badge) / 2, baseline, size: sz.badge, family: FONT_MONO, weight: 500, color: ink, mono: true });
          cx += 10 + 4;
        }
        runs.push({ text: b.label, x: cx, baseline, size: sz.badge, family: FONT_MONO, weight: 500, color: ink, spacing: 1, mono: true });
      }
    });
  }
  return { runs, boxes, name, since, number };
}
function badgeOf(b) {
  const style = b.style || BADGE_STYLES[b.id] || "ink";
  const glyph = b.glyph || (b.icon ? BADGE_GLYPHS[b.icon] || null : null);
  return { label: String(b.name || b.label || b.id || "").toUpperCase(), style, glyph: glyph ? [...glyph][0] : null };
}
function badgeWidth(b) {
  return 8 + (b.glyph ? 10 + 4 : 0) + monoWidth(b.label, TOKENS.size.badge, 1) + 8 + (b.style === "outline" ? 2 : 0);
}
export function captionOf(art) {
  return `${String(art.number).padStart(2, "0")} — ${art.title} · ${art.finish}`;
}

// The projective map of a card tilted `rx`, `ry` radians about its centre,
// seen from `perspective` points away (zgx's LocalTransform::projective),
// as a column-major 3x3 for the vertex shader.
function tiltMatrix(rx, ry, perspective, w, h) {
  const ox = w / 2, oy = h / 2;
  const sx = Math.sin(rx), cx = Math.cos(rx), sy = Math.sin(ry), cy = Math.cos(ry);
  const depth = perspective > 0 ? 1 / perspective : 0;
  // rows of T(o) · tilt · T(-o)
  const a = cy, b = sx * sy, c = 0, d = cx, px = cx * sy * depth, py = -sx * depth;
  // x' = a(x-ox) + c(y-oy) + ox·w ; y' = b(x-ox) + d(y-oy) + oy·w ; w = px(x-ox) + py(y-oy) + 1
  const m = [
    a + ox * px, b + oy * px, px,
    c + ox * py, d + oy * py, py,
    -a * ox - c * oy + ox * (1 - px * ox - py * oy), -b * ox - d * oy + oy * (1 - px * ox - py * oy), 1 - px * ox - py * oy
  ];
  return new Float32Array(m);
}

export function mountCard(container, card, options = {}) {
  const opts = { theme: "auto", caption: false, arrive: false, still: "auto", scale: "fit", origin: null, dpr: null, rig: null, shadow: true, ...options };
  const art = card.art;
  if (!art || !art.layers || !art.layers.length) throw new Error("the card needs its art (artFor)");
  const reduced = typeof matchMedia === "function" && matchMedia("(prefers-reduced-motion: reduce)").matches;
  const still = opts.still === "auto" ? reduced : !!opts.still;
  const arriving = !!opts.arrive && !reduced && !opts.rig;
  const finish = Math.max(0, HOLOS.indexOf(card.holo));
  // Where the character field's grid starts: the card's \`sky\`, within one
  // cell (the native card's \`sky_origin\`), so two cards of one scene read
  // differently.
  const sky = Number(card.sky) >>> 0;
  const skyOrigin = [sky % 9, Math.floor(sky / 9) % 12];
  const number = Number(card.userNumber) || 0;
  const layout = layoutCard(card);
  const caption = opts.caption ? captionOf(art) : null;

  const canvas = document.createElement("canvas");
  canvas.className = "zeron-card";
  canvas.style.display = "block";
  canvas.style.touchAction = "none";
  container.appendChild(canvas);

  const state = {
    ready: false, failed: false,
    frame: { w: 0, h: 0 }, dpr: 1, z: 1, origin: { x: 0, y: 0 },
    px: 0.5, py: 0.5, inside: false, lit: false,
    strength: new Tween(0, 140, outQuad),
    rotX: new Spring(0, 220, 28), rotY: new Spring(0, 220, 28),
    shift: art.layers.map(() => ({ x: new Spring(0, 160, 22), y: new Spring(0, 160, 22) })),
    foilAt: [0.5, 0.5], sweep: null,
    arrive: arriving ? { t0: null } : null, arrived: !arriving,
    mounted: performance.now(), last: performance.now(), renderMs: [], frameGapMs: [], lastFrame: null,
    timers: [], raf: 0, visible: true, lost: false
  };
  const controller = {
    canvas, state, card, layout,
    ready: null,
    destroy() {
      cancelAnimationFrame(state.raf);
      for (const t of state.timers) clearTimeout(t);
      observer?.disconnect(); visibility?.disconnect();
      document.removeEventListener("visibilitychange", onVisibility);
      canvas.remove();
    },
    // For captures: a frozen pose. `pose` [x, y] on the card (0 to 1) or null
    // for the pointer away; `lit`, `strength`; `drift` an array of offsets in
    // points per layer or the milliseconds since mount; `tilt` false keeps the
    // card flat in that pose (the foil and parallax at the pointer, no turn).
    rig(r) { applyRig(r); render(); },
    tick: null
  };

  let gfx = null, observer = null, visibility = null;
  let textures = { layers: [], sky: null, glyphs: null, words: null, caption: null, cardFbo: null };
  let sizes = null;

  function themeOf() {
    if (opts.theme === "dark" || opts.theme === "light") return THEMES[opts.theme];
    return typeof matchMedia === "function" && matchMedia("(prefers-color-scheme: light)").matches ? THEMES.light : THEMES.dark;
  }

  // The frame: the container's box; the card scaled to fit it with room for
  // its shadow, or at the scale asked.
  function measure() {
    const rect = container.getBoundingClientRect();
    const fw = Math.max(1, Math.round(rect.width)), fh = Math.max(1, Math.round(rect.height));
    const dpr = opts.dpr || window.devicePixelRatio || 1;
    const captionH = caption ? PAD + lineOf(TOKENS.size.caption) : 0;
    let z = opts.scale === "fit" ? Math.min(1.25, (fw - 48) / W, (fh - 24 - 56) / (H + captionH)) : Number(opts.scale) || 1;
    z = Math.max(z, 0.1);
    const origin = opts.origin ? { x: opts.origin.x, y: opts.origin.y } : { x: (fw - W * z) / 2, y: (fh - (H + captionH) * z) / 2 - 8 * z };
    const changed = fw !== state.frame.w || fh !== state.frame.h || dpr !== state.dpr || z !== state.z;
    state.frame = { w: fw, h: fh }; state.dpr = dpr; state.z = z; state.origin = origin;
    canvas.style.width = `${fw}px`; canvas.style.height = `${fh}px`;
    const bw = Math.round(fw * dpr), bh = Math.round(fh * dpr);
    if (canvas.width !== bw || canvas.height !== bh) { canvas.width = bw; canvas.height = bh; }
    return changed;
  }

  function loadImage(src) {
    return new Promise((resolve, reject) => {
      const img = new Image();
      img.decoding = "async";
      img.onload = () => resolve(img);
      img.onerror = () => reject(new Error(`could not load ${src}`));
      img.src = src;
    });
  }

  async function loadFonts() {
    if (!document.fonts) return;
    const want = [`700 16px "${FONT_SANS}"`, `600 34px "${FONT_SANS}"`, `400 12px "${FONT_MONO}"`, `500 10px "${FONT_MONO}"`];
    await Promise.all(want.map((f) => document.fonts.load(f).catch(() => null)));
  }

  // The static pictures at the card's current scale: the sky through its
  // characters, the words, the caption.
  function buildStatic() {
    const gl = gfx.gl;
    const s = state.z * state.dpr;
    for (const key of ["glyphs", "words", "caption", "cardFbo"]) {
      const t = textures[key];
      if (!t) continue;
      if (t.fbo) gl.deleteFramebuffer(t.fbo);
      gl.deleteTexture(t.tex || t);
      textures[key] = null;
    }
    const node = [W - 2 * BORDER, H - 2 * BORDER];
    // The still sky: the layers at depth 0, as painted, over the node's box.
    const sky = gfx.target(node[0], node[1], s, [-BORDER, -BORDER]);
    gfx.bind(sky); gfx.clear();
    art.layers.forEach((l, i) => {
      if (l.depth !== 0) return;
      const r = layerRect(i, 0, 0);
      gfx.image(textures.layers[i].tex, r.x, r.y, r.w, r.h);
    });
    // Through the characters.
    const glyphs = gfx.target(node[0], node[1], s, [-BORDER, -BORDER]);
    gfx.bind(glyphs); gfx.clear();
    gfx.draw("sky", BORDER, BORDER, node[0], node[1], (u) => {
      gl.activeTexture(gl.TEXTURE0); gl.bindTexture(gl.TEXTURE_2D, sky.tex); gl.uniform1i(u.u_tex, 0);
      gl.uniform2f(u.u_node, node[0], node[1]); gl.uniform1f(u.u_px, 1.5);
      gl.uniform2f(u.u_grid, skyOrigin[0], skyOrigin[1]);
    });
    gl.deleteFramebuffer(sky.fbo); gl.deleteTexture(sky.tex);
    textures.sky = null;
    textures.glyphs = glyphs;
    // The words, set by the browser.
    const words = document.createElement("canvas");
    words.width = glyphs.width; words.height = glyphs.height;
    const ctx = words.getContext("2d");
    for (const run of layout.runs) fillRun(ctx, s, { ...run, x: run.x - BORDER, baseline: run.baseline - BORDER });
    textures.words = gfx.upload(words);
    if (caption) {
      const size = TOKENS.size.caption, line = lineOf(size);
      const cw = monoWidth(caption, size, 3);
      const c = document.createElement("canvas");
      c.width = Math.max(1, Math.ceil(cw * s)); c.height = Math.max(1, Math.ceil(line * s));
      fillRun(c.getContext("2d"), s, { text: caption, x: 0, baseline: baselineIn(0, line, size), size, family: FONT_MONO, weight: 400, color: themeOf().caption, spacing: 3, mono: true });
      textures.caption = { tex: gfx.upload(c), w: cw, h: line };
    }
    textures.cardFbo = gfx.target(W + 2 * MARGIN, H + 2 * MARGIN, s, [MARGIN, MARGIN]);
    sizes = { s };
  }

  // Layer `i`'s picture: its box hangs 24 and 16 past the node, the picture
  // covering it (scaled up to fill, centred), shifted by `dx`, `dy`.
  function layerRect(i, dx, dy) {
    const img = textures.layers[i];
    const bw = W - 2 * BORDER + 48, bh = H - 2 * BORDER + 32;
    const scale = Math.max(bw / img.width, bh / img.height);
    const w = img.width * scale, h = img.height * scale;
    return { x: BORDER - 24 + (bw - w) / 2 + dx, y: BORDER - 16 + (bh - h) / 2 + dy, w, h };
  }

  // A layer's drift: anime's alternate loop between -5d and 5d on inOutSine,
  // over 16000/d ms plus the member's number's share.
  function driftOf(depth, ms) {
    if (depth <= 0 || still) return 0;
    const duration = 16000 / depth + (number % 5) * 900;
    const cycles = ms / duration;
    let p = cycles - Math.floor(cycles);
    if (Math.floor(cycles) % 2 === 1) p = 1 - p;
    return lerp(-depth * 5, depth * 5, inOutSine(p));
  }

  const clip = { x: BORDER, y: BORDER, w: W - 2 * BORDER, h: H - 2 * BORDER, r: RADIUS - BORDER };

  function render(now = performance.now()) {
    if (!gfx || state.lost) return;
    const began = performance.now();
    const gl = gfx.gl;
    const s = state.z * state.dpr;
    if (!sizes || sizes.s !== s) buildStatic();
    const r = state.rigged;
    const ms = r && r.drift != null ? (Array.isArray(r.drift) ? null : r.drift) : now - state.mounted;
    const arriveT = state.arrive && state.arrive.t0 != null ? Math.min(1, (now - state.arrive.t0) / 820) : 1;
    const opacity = state.arrive ? (state.arrive.t0 == null ? 0 : outCubic(arriveT)) : 1;
    const lift = state.arrive ? (state.arrive.t0 == null ? 36 : lerp(36, 0, outCubic(arriveT))) : 0;
    const node = [W - 2 * BORDER, H - 2 * BORDER];

    // The card, into its own picture.
    const cardFbo = textures.cardFbo;
    gfx.bind(cardFbo); gfx.clear();
    gfx.image(textures.glyphs.tex, BORDER, BORDER, node[0], node[1], { clip, opacity });
    art.layers.forEach((l, i) => {
      if (l.depth === 0) return;
      const drift = r && Array.isArray(r.drift) ? (r.drift[l.depth - 1] ?? 0) : driftOf(l.depth, ms);
      const sh = state.shift[i];
      const rect = layerRect(i, (still ? 0 : sh.x.x) + drift, still ? 0 : sh.y.x);
      gfx.image(textures.layers[i].tex, rect.x, rect.y, rect.w, rect.h, { clip, opacity });
    });
    gfx.draw("paper", BORDER, BORDER, node[0], node[1], (u) => {
      gl.uniform2f(u.u_node, node[0], node[1]);
      gl.uniform3f(u.u_paper, TOKENS.paper[0] / 255, TOKENS.paper[1] / 255, TOKENS.paper[2] / 255);
      gl.uniform1fv(u.u_stops, new Float32Array(PAPER_STOPS.flatMap(([t, a]) => [t, a / 255])));
    }, { clip, opacity });
    for (const b of layout.boxes) drawBox(b, opacity);
    gfx.image(textures.words, BORDER, BORDER, node[0], node[1], { clip, opacity });
    if (state.lit || (r && r.lit)) {
      const k = r && r.strength != null ? r.strength : state.strength.x;
      const at = state.foilAt;
      gfx.draw("foil", BORDER, BORDER, node[0], node[1], (u) => {
        gl.uniform2f(u.u_node, node[0], node[1]); gl.uniform2f(u.u_at, at[0], at[1]);
        gl.uniform1f(u.u_k, 1); gl.uniform1i(u.u_finish, finish);
      }, { clip, opacity: k * opacity });
    }
    gfx.draw("ring", 0, 0, W, H, (u) => {
      gl.uniform4f(u.u_box, 0, 0, W, H); gl.uniform1f(u.u_radius, RADIUS); gl.uniform1f(u.u_width, BORDER);
      gl.uniform4f(u.u_color, TOKENS.edge[0] / 255, TOKENS.edge[1] / 255, TOKENS.edge[2] / 255, TOKENS.edge[3] / 255);
    }, { opacity });

    // The screen: the shadow under the card, the card tilted, the caption.
    const screen = { fbo: null, width: canvas.width, height: canvas.height, s, origin: [state.origin.x / state.z, state.origin.y / state.z + lift], flip: 1 };
    gfx.bind(screen); gfx.clear();
    if (opts.shadow) {
      const sh = TOKENS.shadow;
      const core = { x: sh.x, y: sh.y, w: W, h: H };
      const reach = sh.sigma * 3;
      gfx.draw("shadow", core.x - reach, core.y - reach, core.w + 2 * reach, core.h + 2 * reach, (u) => {
        gl.uniform4f(u.u_box, core.x, core.y, core.w, core.h); gl.uniform1f(u.u_radius, RADIUS); gl.uniform1f(u.u_sigma, sh.sigma);
        gl.uniform4f(u.u_color, sh.color[0] / 255, sh.color[1] / 255, sh.color[2] / 255, sh.color[3] / 255);
      }, { opacity });
    }
    const flat = still || (r && r.tilt === false);
    const rx = flat ? 0 : state.rotX.x, ry = flat ? 0 : state.rotY.x;
    const m = rx !== 0 || ry !== 0 ? tiltMatrix((rx * Math.PI) / 180, (ry * Math.PI) / 180, 900, W, H) : null;
    gfx.image(cardFbo.tex, -MARGIN, -MARGIN, W + 2 * MARGIN, H + 2 * MARGIN, { m });
    if (textures.caption) {
      const c = textures.caption;
      gfx.image(c.tex, (W - c.w) / 2, H + PAD, c.w, c.h);
    }
    gl.flush();
    // The last frames' cost on the main thread (the GPU's own is apart).
    state.renderMs.push(performance.now() - began);
    if (state.renderMs.length > 240) state.renderMs.shift();
    if (state.lastFrame != null) { state.frameGapMs.push(now - state.lastFrame); if (state.frameGapMs.length > 240) state.frameGapMs.shift(); }
    state.lastFrame = now;
  }

  function drawBox(b, opacity) {
    const gl = gfx.gl;
    gfx.draw("box", b.x - 1, b.y - 1, b.w + 2, b.h + 2, (u) => {
      gl.uniform4f(u.u_box, b.x, b.y, b.w, b.h); gl.uniform1f(u.u_radius, b.r); gl.uniform1f(u.u_border, b.border || 0);
      const f = b.fill || [0, 0, 0, 0], bc = b.borderColor || [0, 0, 0, 0];
      gl.uniform4f(u.u_fill, f[0] / 255, f[1] / 255, f[2] / 255, f[3] / 255);
      gl.uniform4f(u.u_borderColor, bc[0] / 255, bc[1] / 255, bc[2] / 255, bc[3] / 255);
      gl.uniform1f(u.u_grad, b.gradient ? 1 : 0);
      if (b.gradient) {
        gl.uniform4fv(u.u_gstops, new Float32Array(b.gradient.stops.flatMap((c) => [c[0] / 255, c[1] / 255, c[2] / 255, c[3] / 255])));
        gl.uniform1f(u.u_gangle, b.gradient.angle);
      }
    }, { clip, opacity });
  }

  // The pointer's place on the card, -0.5 to 0.5 each way, aimed at: the
  // springs chase it, the foil is there at once.
  function aim(x, y) {
    state.px = x + 0.5; state.py = y + 0.5;
    state.rotX.goal = lerp(TILT, -TILT, state.py);
    state.rotY.goal = lerp(-TILT, TILT, state.px);
    art.layers.forEach((l, i) => {
      state.shift[i].x.goal = lerp(l.depth * 8, -l.depth * 8, state.px);
      state.shift[i].y.goal = lerp(l.depth * 5, -l.depth * 5, state.py);
    });
    state.foilAt = [state.px, state.py];
  }
  function later(ms, fn) { const t = setTimeout(() => { state.timers = state.timers.filter((x) => x !== t); fn(); }, ms); state.timers.push(t); }
  function hover(inside) {
    state.inside = inside;
    if (inside) {
      state.lit = true;
      later(16, () => { if (state.inside) state.strength.set(1, performance.now()); });
    } else {
      aim(0, 0);
      state.strength.set(0, performance.now());
      later(260, () => { if (!state.inside) state.lit = false; });
    }
    wake();
  }
  function cardPoint(e) {
    const rect = canvas.getBoundingClientRect();
    const x = (e.clientX - rect.left - state.origin.x) / state.z;
    const y = (e.clientY - rect.top - state.origin.y) / state.z;
    return { x, y, inside: x >= 0 && x < W && y >= 0 && y < H };
  }
  function onPointer(e) {
    if (state.rigged) return;
    const p = cardPoint(e);
    if (e.type === "pointerleave" || e.type === "pointercancel" || (e.type === "pointerup" && e.pointerType !== "mouse")) {
      if (state.inside) hover(false);
      return;
    }
    if (p.inside !== state.inside) hover(p.inside);
    if (p.inside && !still) { aim(p.x / W - 0.5, p.y / H - 0.5); wake(); }
    else if (p.inside) { state.foilAt = [p.x / W, p.y / H]; wake(); }
  }
  canvas.addEventListener("pointermove", onPointer);
  canvas.addEventListener("pointerdown", onPointer);
  canvas.addEventListener("pointerup", onPointer);
  canvas.addEventListener("pointerleave", onPointer);
  canvas.addEventListener("pointercancel", onPointer);

  function applyRig(r) {
    state.rigged = r || null;
    if (!r) return;
    if (r.pose) { aim(r.pose[0] - 0.5, r.pose[1] - 0.5); } else { aim(0, 0); }
    for (const sp of [state.rotX, state.rotY, ...state.shift.flatMap((s) => [s.x, s.y])]) { sp.x = sp.goal; sp.v = 0; }
    state.lit = !!r.lit; state.strength.x = state.strength.goal = r.strength ?? (r.lit ? 1 : 0);
    state.inside = !!r.pose;
    state.arrive = null; state.arrived = true;
  }

  // What keeps moving: the springs, the strength, the drift, the arrival.
  function moving() {
    if (state.rigged) return false;
    if (state.arrive && !state.arrived) return true;
    if (state.sweep) return true;
    if (!state.strength.settled()) return true;
    if (!still && (!state.rotX.settled() || !state.rotY.settled() || state.shift.some((s) => !s.x.settled() || !s.y.settled()))) return true;
    if (!still && art.layers.some((l) => l.depth > 0)) return true;
    return false;
  }
  function wake() { if (!state.raf && state.visible && state.ready) state.raf = requestAnimationFrame(tick); }
  function tick(now) {
    state.raf = 0;
    const dt = Math.max(0, (now - state.last) / 1000);
    state.last = now;
    if (!still) {
      state.rotX.step(dt); state.rotY.step(dt);
      for (const sh of state.shift) { sh.x.step(dt); sh.y.step(dt); }
    }
    state.strength.step(now);
    if (state.sweep) {
      const t = (now - state.sweep.t0 - 120) / 900;
      if (t >= 1) { state.foilAt = [1.4, 0.45]; state.sweep = null; }
      else if (t > 0) { const e = inOutSine(t); state.foilAt = [lerp(-0.4, 1.4, e), lerp(0.5, 0.45, e)]; }
      if (state.inside) state.sweep = null;
    }
    if (state.arrive && state.arrive.t0 != null && now - state.arrive.t0 >= 820 && !state.arrived) { state.arrived = true; state.arrive = null; }
    render(now);
    if (moving()) state.raf = requestAnimationFrame(tick);
  }
  controller.tick = tick;

  function onVisibility() {
    state.visible = document.visibilityState !== "hidden" && (!visibility || state.seen !== false);
    if (state.visible) { state.last = performance.now(); wake(); }
    else if (state.raf) { cancelAnimationFrame(state.raf); state.raf = 0; }
  }
  document.addEventListener("visibilitychange", onVisibility);
  if (typeof IntersectionObserver === "function") {
    visibility = new IntersectionObserver((entries) => { state.seen = entries.some((e) => e.isIntersecting); onVisibility(); }, { threshold: 0 });
    visibility.observe(canvas);
  }
  if (typeof ResizeObserver === "function") {
    observer = new ResizeObserver(() => { if (measure() && state.ready) render(); });
    observer.observe(container);
  }
  canvas.addEventListener("webglcontextlost", (e) => { e.preventDefault(); state.lost = true; if (state.raf) { cancelAnimationFrame(state.raf); state.raf = 0; } });
  canvas.addEventListener("webglcontextrestored", () => { state.lost = false; try { setupGl(); render(); wake(); } catch (err) { fail(err); } });

  function setupGl() {
    gfx = new Gfx(canvas);
    textures.layers = textures.images.map((img) => ({ tex: gfx.upload(img), width: img.naturalWidth || img.width, height: img.naturalHeight || img.height }));
    sizes = null;
  }
  function fail(err) {
    state.failed = true;
    canvas.remove();
    container.dispatchEvent(new CustomEvent("cardfailed", { detail: err }));
  }

  controller.ready = (async () => {
    try {
      measure();
      const [images] = await Promise.all([Promise.all(art.layers.map((l) => loadImage(l.src))), loadFonts()]);
      textures.images = images;
      setupGl();
      if (opts.rig) applyRig(opts.rig);
      state.ready = true;
      if (state.arrive) {
        // Dealt onto the screen: up from below, its foil sweeping once.
        state.arrive.t0 = performance.now();
        state.lit = true;
        later(16, () => { state.strength.set(1, performance.now()); state.sweep = { t0: performance.now() }; });
        later(1000, () => { if (!state.inside) state.strength.set(0, performance.now()); });
        later(1200, () => { if (!state.inside) state.lit = false; });
      }
      render();
      wake();
      container.dispatchEvent(new CustomEvent("cardready"));
      return controller;
    } catch (err) {
      fail(err);
      throw err;
    }
  })();
  return controller;
}
