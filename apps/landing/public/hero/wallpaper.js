// A new chat's wallpaper, as the Zeron app draws it: the Parallax scenes
// plugin's Blossom scene (apps/zgx/plugins-builtin/parallax-scenes) under
// Zeron's ASCII effect (apps/zgx/ui/ds/effects.luau `ascii`), faded from the
// top, full, to nothing at its foot (ds/wallpaper.luau). WebGL2.
//
// How zgx draws it, and so how this does:
// - The wallpaper's box (`#wallpaper-layer`) is a layer: its content (the
//   scene's four pictures, each `fit = "cover"` in a box overhanging the place
//   by as far as the nearest moves) is drawn into a texture at device pixels,
//   the box's `fade` (a bottom band the box's height, smoothstep: zgx-gpu
//   draw.wgsl `faded`) cutting the layer as it composites (the scene as one
//   picture, faded once, as the app's pixels show). Pictures are premultiplied and
//   sampled linearly; blending is in encoded sRGB, as zgx's Bgra8Unorm
//   surface does.
// - The layer composites through the ASCII layer shader, ported line for
//   line: `zgx_sample` reads the layer's texel bilinearly at a point and
//   un-premultiplies it.

const VS = `#version 300 es
in vec2 a_unit;
uniform vec4 u_rect;      // device px: x, y, w, h (top-left origin)
uniform vec2 u_target;    // device px of the target
out vec2 v_unit;
void main() {
  v_unit = a_unit;
  vec2 p = u_rect.xy + a_unit * u_rect.zw;
  gl_Position = vec4(p.x / u_target.x * 2. - 1., 1. - p.y / u_target.y * 2., 0., 1.);
}`;

// A picture into the layer: premultiplied, times the fade at the pixel.
const FS_PICTURE = `#version 300 es
precision highp float;
in vec2 v_unit;
uniform sampler2D u_tex;
uniform vec4 u_uv;        // uv at the quad's top-left, and across it
uniform float u_scale;    // device px a point
uniform vec2 u_origin;    // the layer's top-left, points (window)
uniform vec4 u_fade;      // fade box top (pt, window), bottom, bottom band
uniform vec2 u_target;
out vec4 o;
float faded(float y) {
  if (u_fade.z <= 0.) return 1.;
  float t = clamp((u_fade.y - y) / u_fade.z, 0., 1.);
  return t * t * (3. - 2. * t);
}
void main() {
  vec4 t = texture(u_tex, u_uv.xy + v_unit * u_uv.zw);
  // (gl_FragCoord is bottom-up in the layer's framebuffer.)
  float y = u_origin.y + (u_target.y - gl_FragCoord.y) / u_scale;
  o = t * faded(y);
}`;

// zgx-gpu decode.luau GLYPHS: the 5x7 font of " .-+xXS80#@".
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
}`;

// The layer composited through ds/effects.luau `ascii`, clipped to the
// page card's inside (a rounded box, zgx's `rounded` coverage).
const FS_ASCII = `#version 300 es
precision highp float;
precision highp int;
uniform sampler2D u_layer;
uniform vec2 u_target;    // device px of the canvas
uniform float u_scale;    // device px a point
uniform vec2 u_offset;    // the window point at the canvas' top-left (a crop)
uniform vec4 u_node;      // the layer's box, points (window)
uniform vec3 u_params;    // font pixel (pt), strength, threshold
uniform vec4 u_clip;      // rounded clip box, points
uniform float u_radius;
uniform float u_effect;   // 1: ASCII; 0: the picture as it is
uniform vec4 u_fade;      // the fade's box top, bottom, bottom band (points)
out vec4 o;
float faded(float y) {
  if (u_fade.z <= 0.) return 1.;
  float t = clamp((u_fade.y - y) / u_fade.z, 0., 1.);
  return t * t * (3. - 2. * t);
}
${GLYPHS}
vec4 zgx_sample(vec2 p) {
  // (Past the layer's box its texture is clear: the app's last, partial
  // column of cells reads nothing there and holds no glyph.)
  if (any(lessThan(p, vec2(0.))) || any(greaterThan(p, u_node.zw))) return vec4(0.);
  vec2 lo = vec2(.5) / u_scale, hi = u_node.zw - vec2(.5) / u_scale;
  vec2 uv = clamp(p, lo, hi) / u_node.zw;
  vec4 t = texture(u_layer, vec2(uv.x, 1. - uv.y));
  return t.a > 0. ? vec4(t.rgb / t.a, t.a) : vec4(0.);
}
float rounded(vec4 b, float r, vec2 p) {
  vec2 half_ = b.zw * .5; vec2 q = p - b.xy - half_;
  vec2 d = abs(q) - half_ + vec2(r);
  float sdf = length(max(d, vec2(0.))) + min(max(d.x, d.y), 0.) - r;
  return clamp(.5 - sdf * u_scale, 0., 1.);
}
void main() {
  vec2 point = vec2(gl_FragCoord.x, u_target.y - gl_FragCoord.y) / u_scale + u_offset;
  vec2 p = point - u_node.xy;
  vec4 color = zgx_sample(p);
  vec4 c4;
  if (u_effect < .5) {
    c4 = color;
  } else {
    float px = max(1., floor(u_params.x * u_scale + .5)) / u_scale;
    vec2 size = vec2(6., 8.) * px;
    vec2 cell = floor(p / size);
    vec4 c = zgx_sample((cell + .5) * size);
    float lum = dot(c.rgb, vec3(.2126, .7152, .0722));
    float peak = max(max(c.r, c.g), c.b);
    float t = u_params.z;
    float level = pow(clamp((lum - t) / max(1. - t, .01), 0., 1.), 1.4);
    uint i = uint(level * 10.999);
    vec3 dim = color.rgb * .75;
    if (i == 0u) {
      c4 = vec4(mix(color.rgb, dim, u_params.y), color.a);
    } else {
      uvec2 g = glyph(i);
      uvec2 fp = uvec2(floor((p - cell * size) / px));
      uint lit = 0u;
      if (fp.x < 5u && fp.y < 7u) lit = ((fp.y < 6u ? g.x >> (fp.y * 5u) : g.y) >> (4u - fp.x)) & 1u;
      vec3 art = lit == 1u ? c.rgb / max(peak, .004) : dim;
      c4 = vec4(mix(color.rgb, art, u_params.y), color.a);
    }
  }
  c4 = clamp(c4, 0., 1.);
  float k = rounded(u_clip, u_radius, point) * faded(point.y);
  o = vec4(c4.rgb * c4.a, c4.a) * k;
}`;

// A sprite: a texture (premultiplied) over a quad placed by an affine map
// (device px), times an opacity.
const VS_SPRITE = `#version 300 es
in vec2 a_unit;
uniform vec2 u_size;      // the quad, device px
uniform mat3 u_map;       // device px of the quad → device px of the target
uniform vec2 u_target;
out vec2 v_unit;
void main() {
  v_unit = a_unit;
  vec2 p = (u_map * vec3(a_unit * u_size, 1.)).xy;
  gl_Position = vec4(p.x / u_target.x * 2. - 1., 1. - p.y / u_target.y * 2., 0., 1.);
}`;
const FS_SPRITE = `#version 300 es
precision highp float;
in vec2 v_unit;
uniform sampler2D u_tex;
uniform float u_alpha;
out vec4 o;
void main() { o = texture(u_tex, v_unit) * u_alpha; }`;

// zeron/zgx's wallpaper effects (apps/zgx/ui/ds/effects.luau there): the
// ASCII effect as that branch draws it, ported line for line. The layer's
// box is the art's (the window's width from the page's right edge); its
// glyph cells anchor there, the fade runs over its height, and the page
// card clips it. Its params as ds/wallpaper.luau gives them at Medium
// (strength 1, density 0), dark, faded, no picture's grid (a plugin's art).
const FS_ASCII2 = `#version 300 es
precision highp float;
precision highp int;
uniform sampler2D u_layer;
uniform vec2 u_target;
uniform float u_scale;
uniform vec2 u_offset;
uniform vec4 u_node;
uniform vec3 u_params;    // strength, density, faded
uniform vec4 u_clip;
uniform float u_radius;
out vec4 o;
const vec3 FX_PAPER_DARK = vec3(0.043, 0.039, 0.055);
const uint FX_N = 9u;
const uint FX_RAMP[63] = uint[63](0u,0u,0u,0u,0u,0u,0u,
  0u,0u,0u,0u,0u,4u,0u,  0u,4u,0u,0u,4u,0u,0u,  0u,0u,0u,14u,0u,0u,0u,  0u,0u,14u,0u,14u,0u,0u,
  0u,4u,4u,31u,4u,4u,0u,  0u,21u,14u,31u,14u,21u,0u,  10u,10u,31u,10u,31u,10u,10u,  14u,17u,23u,21u,23u,16u,14u);
const float FX_PX = 1.1, FX_GAMMA = .8, FX_T = .1, FX_VIVID = .5, FX_LIFT = .22, FX_K = .34, FX_GROUND = .3, FX_DIM = .75;
vec4 zgx_sample(vec2 p) {
  if (any(lessThan(p, vec2(0.))) || any(greaterThan(p, u_node.zw))) return vec4(0.);
  vec2 lo = vec2(.5) / u_scale, hi = u_node.zw - vec2(.5) / u_scale;
  vec2 uv = clamp(p, lo, hi) / u_node.zw;
  vec4 t = texture(u_layer, vec2(uv.x, 1. - uv.y));
  return t.a > 0. ? vec4(t.rgb / t.a, t.a) : vec4(0.);
}
vec3 fx_lin(vec3 c) { vec3 x = clamp(c, 0., 1.); return mix(pow((x + .055) / 1.055, vec3(2.4)), x / 12.92, vec3(lessThanEqual(x, vec3(.04045)))); }
vec3 fx_srgb(vec3 c) { vec3 x = clamp(c, 0., 1.); return mix(1.055 * pow(x, vec3(1. / 2.4)) - .055, x * 12.92, vec3(lessThanEqual(x, vec3(.0031308)))); }
vec3 fx_a_at(vec2 p) { return zgx_sample(clamp(p, vec2(0.), u_node.zw - vec2(.01))).rgb; }
float fx_px() { float s = max(u_scale, 1.); return max(floor(FX_PX * s + .5), 1.) / s; }
vec3 fx_a_pix(ivec2 k) {
  ivec2 cell = ivec2(floor(vec2(k) / vec2(6., 8.)));
  vec3 c = fx_a_at((vec2(cell) + .5) * vec2(6., 8.) * fx_px());
  float lum = dot(c, vec3(.2126, .7152, .0722));
  float t = clamp(FX_T + u_params.y, 0., .9);
  float level = pow(clamp((lum - t) / (1. - t), 0., 1.), FX_GAMMA);
  uint i = min(uint(level * (float(FX_N) - .001)), FX_N - 1u);
  ivec2 at = k - cell * ivec2(6, 8);
  bool lit = false;
  if (i > 0u && at.x < 5 && at.y < 7) lit = ((FX_RAMP[i * 7u + uint(at.y)] >> uint(4 - at.x)) & 1u) == 1u;
  if (lit) {
    float peak = max(max(c.r, c.g), max(c.b, .004));
    return mix(mix(c, c / peak, FX_VIVID), vec3(1.), FX_LIFT);
  }
  vec3 here = fx_a_at((vec2(k) + .5) * fx_px());
  return mix(FX_PAPER_DARK, here * FX_DIM, FX_GROUND);
}
vec2 fx_over(vec2 x, float w) { vec2 lo = x - .5 * w; return clamp((x + .5 * w - (floor(lo) + 1.)) / max(w, 1e-4), vec2(0.), vec2(1.)); }
float rounded(vec4 b, float r, vec2 p) {
  vec2 half_ = b.zw * .5; vec2 q = p - b.xy - half_;
  vec2 d = abs(q) - half_ + vec2(r);
  float sdf = length(max(d, vec2(0.))) + min(max(d.x, d.y), 0.) - r;
  return clamp(.5 - sdf * u_scale, 0., 1.);
}
void main() {
  vec2 point = vec2(gl_FragCoord.x, u_target.y - gl_FragCoord.y) / u_scale + u_offset;
  vec2 p = point - u_node.xy;
  vec4 color = zgx_sample(p);
  float px = fx_px();
  vec2 x = p / px;
  float w = 1. / (max(u_scale, 1.) * px);
  ivec2 k = ivec2(floor(x - .5 * w));
  vec2 ov = fx_over(x, w);
  vec3 sum = fx_a_pix(k) * (1. - ov.x) * (1. - ov.y);
  if (ov.x > 0.) sum += fx_a_pix(k + ivec2(1, 0)) * ov.x * (1. - ov.y);
  if (ov.y > 0.) sum += fx_a_pix(k + ivec2(0, 1)) * (1. - ov.x) * ov.y;
  if (ov.x > 0. && ov.y > 0.) sum += fx_a_pix(k + ivec2(1, 1)) * ov.x * ov.y;
  vec3 e = fx_lin(mix(color.rgb, sum, FX_K));
  vec3 rgb = fx_srgb(mix(fx_lin(color.rgb), e, u_params.x));
  float fade = u_params.z < .5 ? 1. : smoothstep(0., u_node.w, u_node.w - p.y);
  vec4 c4 = clamp(vec4(rgb, color.a * fade), 0., 1.);
  float kc = rounded(u_clip, u_radius, point);
  o = vec4(c4.rgb * c4.a, c4.a) * kc;
}`;

// A texture drawn as it is (premultiplied), over a rect.
const FS_COPY = `#version 300 es
precision highp float;
in vec2 v_unit;
uniform sampler2D u_tex;
out vec4 o;
void main() { o = texture(u_tex, v_unit); }`;

function compile(gl, type, src) {
  const s = gl.createShader(type);
  gl.shaderSource(s, src);
  gl.compileShader(s);
  if (!gl.getShaderParameter(s, gl.COMPILE_STATUS)) throw new Error(gl.getShaderInfoLog(s));
  return s;
}

function program(gl, fs, vs = VS) {
  const p = gl.createProgram();
  gl.attachShader(p, compile(gl, gl.VERTEX_SHADER, vs));
  gl.attachShader(p, compile(gl, gl.FRAGMENT_SHADER, fs));
  gl.bindAttribLocation(p, 0, "a_unit");
  gl.linkProgram(p);
  if (!gl.getProgramParameter(p, gl.LINK_STATUS)) throw new Error(gl.getProgramInfoLog(p));
  const u = {};
  const n = gl.getProgramParameter(p, gl.ACTIVE_UNIFORMS);
  for (let i = 0; i < n; i++) {
    const name = gl.getActiveUniform(p, i).name;
    u[name] = gl.getUniformLocation(p, name);
  }
  return { p, u };
}

export function texture(gl, source, { premultiply = true, nearest = false } = {}) {
  const tex = gl.createTexture();
  gl.bindTexture(gl.TEXTURE_2D, tex);
  gl.pixelStorei(gl.UNPACK_PREMULTIPLY_ALPHA_WEBGL, premultiply);
  gl.pixelStorei(gl.UNPACK_COLORSPACE_CONVERSION_WEBGL, gl.NONE);
  gl.texImage2D(gl.TEXTURE_2D, 0, gl.RGBA, gl.RGBA, gl.UNSIGNED_BYTE, source);
  const filter = nearest ? gl.NEAREST : gl.LINEAR;
  gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_MIN_FILTER, filter);
  gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_MAG_FILTER, filter);
  gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_WRAP_S, gl.CLAMP_TO_EDGE);
  gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_WRAP_T, gl.CLAMP_TO_EDGE);
  return tex;
}

export class Renderer {
  constructor(gl) {
    this.gl = gl;
    this.picture = program(gl, FS_PICTURE);
    this.ascii = program(gl, FS_ASCII);
    this.ascii2 = program(gl, FS_ASCII2);
    this.copy = program(gl, FS_COPY);
    this.sprite = program(gl, FS_SPRITE, VS_SPRITE);
    const vao = gl.createVertexArray();
    gl.bindVertexArray(vao);
    const buf = gl.createBuffer();
    gl.bindBuffer(gl.ARRAY_BUFFER, buf);
    gl.bufferData(gl.ARRAY_BUFFER, new Float32Array([0, 0, 1, 0, 0, 1, 1, 1]), gl.STATIC_DRAW);
    gl.enableVertexAttribArray(0);
    gl.vertexAttribPointer(0, 2, gl.FLOAT, false, 0, 0);
    this.fbo = gl.createFramebuffer();
    this.layer = null;
    this.layerSize = [0, 0];
  }

  // The layer texture at `w`×`h` device px.
  ensureLayer(w, h) {
    const gl = this.gl;
    if (this.layer && this.layerSize[0] === w && this.layerSize[1] === h) return;
    if (this.layer) gl.deleteTexture(this.layer);
    this.layer = gl.createTexture();
    gl.bindTexture(gl.TEXTURE_2D, this.layer);
    gl.texImage2D(gl.TEXTURE_2D, 0, gl.RGBA, w, h, 0, gl.RGBA, gl.UNSIGNED_BYTE, null);
    gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_MIN_FILTER, gl.LINEAR);
    gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_MAG_FILTER, gl.LINEAR);
    gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_WRAP_S, gl.CLAMP_TO_EDGE);
    gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_WRAP_T, gl.CLAMP_TO_EDGE);
    gl.bindFramebuffer(gl.FRAMEBUFFER, this.fbo);
    gl.framebufferTexture2D(gl.FRAMEBUFFER, gl.COLOR_ATTACHMENT0, gl.TEXTURE_2D, this.layer, 0);
    this.layerSize = [w, h];
  }

  // The wallpaper's layer: `pictures` ({ tex, w, h, box: [x, y, w, h] in
  // points, window }) covering their boxes, clipped to `node` (points),
  // faded by `fade` (bottom band, points; 0 none).
  drawLayer(scale, node, pictures, fade) {
    const gl = this.gl;
    const w = Math.max(1, Math.round(node[2] * scale)), h = Math.max(1, Math.round(node[3] * scale));
    this.ensureLayer(w, h);
    gl.bindFramebuffer(gl.FRAMEBUFFER, this.fbo);
    gl.viewport(0, 0, w, h);
    gl.clearColor(0, 0, 0, 0);
    gl.clear(gl.COLOR_BUFFER_BIT);
    gl.enable(gl.BLEND);
    gl.blendFunc(gl.ONE, gl.ONE_MINUS_SRC_ALPHA);
    const { p, u } = this.picture;
    gl.useProgram(p);
    gl.uniform2f(u.u_target, w, h);
    gl.uniform1f(u.u_scale, scale);
    gl.uniform2f(u.u_origin, node[0], node[1]);
    gl.uniform4f(u.u_fade, node[1], node[1] + node[3], fade, 0);
    gl.activeTexture(gl.TEXTURE0);
    gl.uniform1i(u.u_tex, 0);
    for (const pic of pictures) {
      // `fit = "cover"`: the picture scaled to cover its box, centred, the
      // overflow cut at the box.
      const [bx, by, bw, bh] = pic.box;
      const k = Math.max(bw / pic.w, bh / pic.h);
      const dw = pic.w * k, dh = pic.h * k;
      const dx = bx + (bw - dw) / 2, dy = by + (bh - dh) / 2;
      // The box's part, in uv.
      const u0 = (bx - dx) / dw, v0 = (by - dy) / dh;
      gl.bindTexture(gl.TEXTURE_2D, pic.tex);
      gl.uniform4f(u.u_uv, u0, v0, bw / dw, bh / dh);
      // Layer-relative device px (the layer's y runs down here, flipped when read).
      gl.uniform4f(this.picture.u.u_rect, (bx - node[0]) * scale, (by - node[1]) * scale, bw * scale, bh * scale);
      gl.drawArrays(gl.TRIANGLE_STRIP, 0, 4);
    }
    gl.bindFramebuffer(gl.FRAMEBUFFER, null);
  }

  // A premultiplied texture over the whole canvas.
  drawFull(tex, cw, ch) {
    const gl = this.gl;
    const { p, u } = this.copy;
    gl.useProgram(p);
    gl.viewport(0, 0, cw, ch);
    gl.enable(gl.BLEND);
    gl.blendFunc(gl.ONE, gl.ONE_MINUS_SRC_ALPHA);
    gl.uniform2f(u.u_target, cw, ch);
    gl.uniform4f(u.u_rect, 0, 0, cw, ch);
    gl.activeTexture(gl.TEXTURE0);
    gl.bindTexture(gl.TEXTURE_2D, tex);
    gl.uniform1i(u.u_tex, 0);
    gl.drawArrays(gl.TRIANGLE_STRIP, 0, 4);
  }

  // zeron/zgx's ASCII (FS_ASCII2): strength 1, density 0, faded.
  drawEffect2(cw, ch, scale, node, clip, radius, offset) {
    const gl = this.gl;
    const { p, u } = this.ascii2;
    gl.useProgram(p);
    gl.viewport(0, 0, cw, ch);
    gl.enable(gl.BLEND);
    gl.blendFunc(gl.ONE, gl.ONE_MINUS_SRC_ALPHA);
    gl.uniform2f(u.u_target, cw, ch);
    gl.uniform1f(u.u_scale, scale);
    gl.uniform2f(u.u_offset, offset[0], offset[1]);
    gl.uniform4f(u.u_node, ...node);
    gl.uniform3f(u.u_params, 1, 0, 1);
    gl.uniform4f(u.u_clip, ...clip);
    gl.uniform1f(u.u_radius, radius);
    gl.uniform4f(u.u_rect, (clip[0] - offset[0]) * scale, (clip[1] - offset[1]) * scale, clip[2] * scale, clip[3] * scale);
    gl.activeTexture(gl.TEXTURE0);
    gl.bindTexture(gl.TEXTURE_2D, this.layer);
    gl.uniform1i(u.u_layer, 0);
    gl.drawArrays(gl.TRIANGLE_STRIP, 0, 4);
  }

  // A sprite: `tex` (w×h device px) at `x, y` (device px of its top-left),
  // turned `deg` about `origin` (fractions of it), at `alpha`.
  drawSprite(tex, cw, ch, x, y, w, h, deg = 0, origin = [0.5, 0.5], alpha = 1) {
    const gl = this.gl;
    const { p, u } = this.sprite;
    gl.useProgram(p);
    gl.viewport(0, 0, cw, ch);
    gl.enable(gl.BLEND);
    gl.blendFunc(gl.ONE, gl.ONE_MINUS_SRC_ALPHA);
    const a = (deg * Math.PI) / 180, c = Math.cos(a), s = Math.sin(a);
    const ox = origin[0] * w, oy = origin[1] * h;
    // translate(x + o) · rotate · translate(−o), column-major.
    const tx = x + ox - (c * ox - s * oy), ty = y + oy - (s * ox + c * oy);
    gl.uniformMatrix3fv(u.u_map, false, [c, s, 0, -s, c, 0, tx, ty, 1]);
    gl.uniform2f(u.u_size, w, h);
    gl.uniform2f(u.u_target, cw, ch);
    gl.uniform1f(u.u_alpha, alpha);
    gl.activeTexture(gl.TEXTURE0);
    gl.bindTexture(gl.TEXTURE_2D, tex);
    gl.uniform1i(u.u_tex, 0);
    gl.drawArrays(gl.TRIANGLE_STRIP, 0, 4);
  }

  // The layer composited through the effect, over what's drawn.
  drawEffect(cw, ch, scale, node, params, clip, radius, effect = 1, fade = 0, offset = [0, 0]) {
    const gl = this.gl;
    if (effect === "ascii2") return this.drawEffect2(cw, ch, scale, node, clip, radius, offset);
    const { p, u } = this.ascii;
    gl.useProgram(p);
    gl.viewport(0, 0, cw, ch);
    gl.enable(gl.BLEND);
    gl.blendFunc(gl.ONE, gl.ONE_MINUS_SRC_ALPHA);
    gl.uniform2f(u.u_target, cw, ch);
    gl.uniform1f(u.u_scale, scale);
    gl.uniform2f(u.u_offset, offset[0], offset[1]);
    gl.uniform4f(u.u_node, ...node);
    gl.uniform3f(u.u_params, ...params);
    gl.uniform4f(u.u_clip, ...clip);
    gl.uniform1f(u.u_radius, radius);
    gl.uniform1f(u.u_effect, effect);
    gl.uniform4f(u.u_fade, node[1], node[1] + node[3], fade, 0);
    gl.uniform4f(u.u_rect, (node[0] - offset[0]) * scale, (node[1] - offset[1]) * scale, node[2] * scale, node[3] * scale);
    gl.activeTexture(gl.TEXTURE0);
    gl.bindTexture(gl.TEXTURE_2D, this.layer);
    gl.uniform1i(u.u_layer, 0);
    gl.drawArrays(gl.TRIANGLE_STRIP, 0, 4);
  }
}
