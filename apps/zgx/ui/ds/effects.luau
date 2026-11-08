-- A new chat's wallpaper effects: each a layer shader over the whole
-- picture as it's composited, so the art in it (a picture, or what a
-- plugin draws) takes one pass, not one a layer.
--
-- `ui.div { layer_shader = effects.shader("dither"), layer_params = effects.params { … }, … }`
--
-- Each works in linear light on the picture's own grid (the picture fitted
-- to 2048 a side, as Zeron's proxy of it, so the cells hold to the picture
-- and grow and shrink with it). Halftone and scanlines print each cell
-- with ink on paper so that, seen from a step back, the cell is the
-- picture's own colour there:
--
-- 1. the picture recedes into the paper by one small offset (`RECEDE`);
-- 2. a cell's ink covers a share of it from `FLOOR` to `MOST`, by how far
--    its colour stands from the paper (`fx_share`);
-- 3. the ink lies on the line from the paper through that colour, further
--    out (no more than `PUSH`), and the gaps between make up the rest, so
--    the cell's mean is the colour's exactly (`fx_split`).
--
-- Dither quantizes each cell to two shades of its own colour; ASCII lays
-- glyphs over the picture. The paper follows the scheme (near black in the
-- dark, warm white in the light). Edges are worked out over the window's
-- pixel, and every effect fades the art from its top to nothing at its foot.

local M = {}

-- The longer side of the picture's grid (Zeron's proxy of the picture).
M.GRID_MAX = 2048

-- The paper, by scheme (sRGB): near black, warm white. Fixed, not the
-- card's fill: a theme's grey card under the ink flattened the dark (the
-- texture a fifth to a half fainter), and every theme draws the same.
M.PAPER = { dark = { 0.043, 0.039, 0.055 }, light = { 0.969, 0.961, 0.945 } }
-- How much of a cell the ink covers, from the paper's own colour to the
-- edge of the colours: never less than `FLOOR` (the dark of a picture
-- keeps a quiet texture, not a few bright specks), never more than `MOST`
-- (the ink stays a mark on the paper, the paper taking the rest of the
-- colour where it's bright).
M.FLOOR, M.MOST = 0.12, 0.62
-- By scheme: how far past the colour the ink may go from the paper (2.2:
-- more than twice as far), so it reads at a step back yet stays the
-- picture's colour, deeper, not a printer's primary; and how far every
-- effect lets the picture recede into the paper (in sRGB: the room its
-- ink needs over the brightest of a picture in the dark, the darkest in
-- the light): one offset, the same for all.
M.PUSH = { dark = 2.2, light = 1.7 }
M.RECEDE = { dark = 0.07, light = 0.08 }

local COMMON = string.format(
	[[
// params[0]: strength, density, light (1) or dark (0), faded (1) or not.
// params[1]: the picture's grid (its pixels across, down: 0 for no
//   picture) and where its overflow is cut (0 to 1, 0.5 centred).
// params[2]: its zoom, the ink's least and most share, the recede.
// params[3]: the push.
// The paper, by scheme (sRGB).
const FX_PAPER_DARK=vec3<f32>(%.4f,%.4f,%.4f);
const FX_PAPER_LIGHT=vec3<f32>(%.4f,%.4f,%.4f);
fn fx_paper_srgb(input:LayerInput)->vec3<f32> {return select(FX_PAPER_DARK,FX_PAPER_LIGHT,input.params[0].z>.5);}
// The picture's grid, covering the box: its top left in points, and a
// pixel's size (half a point without a picture).
fn fx_grid(input:LayerInput)->vec3<f32> {
 let pw=input.params[1].x; let ph=input.params[1].y;
 if pw<=0. || ph<=0. {return vec3(0.,0.,.5);}
 let s=max(input.size.x/pw,input.size.y/ph)*max(input.params[2].x,1.);
 return vec3(input.params[1].z*(input.size.x-pw*s),input.params[1].w*(input.size.y-ph*s),s);
}
fn fx_lin(c:vec3<f32>)->vec3<f32> {
 let x=clamp(c,vec3(0.),vec3(1.));
 return select(pow((x+.055)/1.055,vec3(2.4)),x/12.92,x<=vec3(.04045));
}
fn fx_srgb(c:vec3<f32>)->vec3<f32> {
 let x=clamp(c,vec3(0.),vec3(1.));
 return select(1.055*pow(x,vec3(1./2.4))-.055,x*12.92,x<=vec3(.0031308));
}
// The picture at `q` on its grid (in its pixels from its top left), linear,
// receded into the paper by the common offset (as the eye sees it: in
// sRGB, so the dark of a picture isn't lifted more than its light).
fn fx_src(input:LayerInput,g:vec3<f32>,q:vec2<f32>)->vec3<f32> {
 let c=zgx_sample(clamp(g.xy+q*g.z,vec2(0.),input.size-vec2(.01))).rgb;
 return fx_lin(mix(c,fx_paper_srgb(input),input.params[2].w));
}
// Its mean over a box `r` of its pixels across, centred at `q`.
fn fx_mean(input:LayerInput,g:vec3<f32>,q:vec2<f32>,r:vec2<f32>)->vec3<f32> {
 let d=r*.25;
 return .25*(fx_src(input,g,q-d)+fx_src(input,g,q+d)+fx_src(input,g,q+vec2(d.x,-d.y))+fx_src(input,g,q+vec2(-d.x,d.y)));
}
fn fx_paper(input:LayerInput)->vec3<f32> {return fx_lin(fx_paper_srgb(input));}
// How far `c` stands from the paper `p` towards the edge of the colours
// (0 the paper, 1 the edge): the least share of ink that makes `c` up.
fn fx_reach(c:vec3<f32>,p:vec3<f32>)->f32 {
 let up=(c-p)/max(vec3(1.)-p,vec3(1e-4));
 let dn=(p-c)/max(p,vec3(1e-4));
 let m=max(up,dn);
 return clamp(max(max(m.x,m.y),m.z),0.,1.);
}
struct FxInk { ink:vec3<f32>, gap:vec3<f32> }
// Ink covering `k` of a cell, and its gaps, that make `c` up: the ink on
// the line from the paper through `c` (as far as covering `k` takes, no
// further than the push, nor than the colours go), the gaps the paper or,
// where the ink can't make it up alone, the rest of `c` on that line.
fn fx_split(input:LayerInput,c:vec3<f32>,k:f32)->FxInk {
 let p=fx_paper(input);
 let ink=p+(c-p)*min(1./max(max(k,fx_reach(c,p)),1e-4),input.params[3].x);
 return FxInk(ink,(c-k*ink)/max(1.-k,1e-4));
}
// How much of a cell its ink covers where the picture is `c`: from the
// least to the most share as `c` stands from the paper to the edge.
fn fx_share(input:LayerInput,c:vec3<f32>)->f32 {
 return mix(input.params[2].y,input.params[2].z,fx_reach(c,fx_paper(input)));
}
// How much of the art shows here: full at the top, nothing at the foot.
fn fx_alpha(input:LayerInput)->f32 {
 if input.params[0].w<.5 {return 1.;}
 return smoothstep(0.,input.size.y,input.size.y-input.p.y);
}
// What of the window's pixel at `x` (`w` wide, both in cells) falls in the
// next cell over: 0 when it's all in its own.
fn fx_over(x:vec2<f32>,w:f32)->vec2<f32> {
 let lo=x-.5*w;
 return clamp((x+.5*w-(floor(lo)+1.))/max(w,1e-4),vec2(0.),vec2(1.));
}
fn layer(input:LayerInput)->vec4<f32> {
 let g=fx_grid(input);
 let q=(input.p-g.xy)/g.z;
 // The window's pixel, in the picture's.
 let fp=1./(g.z*max(input.scale,1.));
 let e=fx_effect(input,g,q,fp);
 let rgb=fx_srgb(mix(fx_lin(input.color.rgb),e,input.params[0].x));
 return vec4(rgb,input.color.a*fx_alpha(input));
}
]],
	M.PAPER.dark[1],
	M.PAPER.dark[2],
	M.PAPER.dark[3],
	M.PAPER.light[1],
	M.PAPER.light[2],
	M.PAPER.light[3]
)

-- Dither: every cell of 2.5 of the picture's pixels quantized to one of
-- two shades of its own colour, a lighter and a darker, by Bayer's 8 by 8
-- screen. The lighter's share of the cells is the cell's lightness, pulled
-- towards half (so a sky's mid tones and a shadow's both take the weave),
-- and the two shades are `FX_CONTRAST` of as far apart as that share
-- leaves room for (neither past black or white), so the cell's mean is its
-- colour (in sRGB). A gamma `FX_CURVE` before quantizing sets its
-- brightness against None's. The window's pixel takes each cell it covers
-- by how much of it falls there.
local DITHER = [[
const FX_UNIT:f32=2.5;
const FX_CONTRAST:f32=.55;
const FX_CURVE:f32=1.06;
const FX_PULL:f32=.3;
fn fx_threshold(k:vec2<i32>)->f32 {
 let x=u32(k.x); let y=u32(k.y); var v=0u;
 for (var i=0u;i<3u;i++) {v|=(((((x^y)>>i)&1u)<<1u)|((y>>i)&1u))<<(4u-2u*i);}
 return (f32(v)+.5)/64.;
}
fn fx_cell(input:LayerInput,g:vec3<f32>,k:vec2<i32>)->vec3<f32> {
 // The cell's colour, in sRGB, through the curve.
 let c=pow(fx_srgb(fx_mean(input,g,(vec2<f32>(k)+.5)*FX_UNIT,vec2(FX_UNIT))),vec3(FX_CURVE));
 let t=fx_threshold(k);
 let share=clamp(mix(dot(c,vec3(.2126,.7152,.0722)),.5,FX_PULL),.06,.94);
 let room=min((vec3(1.)-c)/(1.-share),c/share);
 let spread=FX_CONTRAST*min(min(room.x,room.y),room.z);
 return fx_lin(select(c-spread*share,c+spread*(1.-share),share>t));
}
fn fx_effect(input:LayerInput,g:vec3<f32>,q:vec2<f32>,fp:f32)->vec3<f32> {
 let x=q/FX_UNIT; let w=fp/FX_UNIT;
 let k=vec2<i32>(floor(x-.5*w));
 let o=fx_over(x,w);
 var s=fx_cell(input,g,k)*(1.-o.x)*(1.-o.y);
 if o.x>0. {s+=fx_cell(input,g,k+vec2(1,0))*o.x*(1.-o.y);}
 if o.y>0. {s+=fx_cell(input,g,k+vec2(0,1))*(1.-o.x)*o.y;}
 if o.x>0. && o.y>0. {s+=fx_cell(input,g,k+vec2(1,1))*o.x*o.y;}
 return s;
}
]]

-- ASCII: glyphs of the 5 by 7 font in cells of 6 by 8 font pixels, each
-- `FX_PX` points (snapped to the device's pixels), anchored to the art's box (still as a sidebar or panel
-- moves). A cell's glyph, of ` .:-=+*#@` (Zeron's ramp less `%`, whose
-- diagonal hatches), rises with the brightness at its centre (in the
-- light, its darkness); cells under the density's threshold hold none.
-- The glyph takes the cell's colour halfway to its full value, lifted a
-- little towards white (in the light, black); between the strokes, the
-- paper with the picture dimmed through it. The art lies over the picture
-- at `FX_K`. All in sRGB.
local RAMP = {
	{ 0, 0, 0, 0, 0, 4, 0 }, -- "."
	{ 0, 4, 0, 0, 4, 0, 0 }, -- ":"
	{ 0, 0, 0, 14, 0, 0, 0 }, -- "-"
	{ 0, 0, 14, 0, 14, 0, 0 }, -- "="
	{ 0, 4, 4, 31, 4, 4, 0 }, -- "+"
	{ 0, 21, 14, 31, 14, 21, 0 }, -- "*"
	{ 10, 10, 31, 10, 31, 10, 10 }, -- "#"
	{ 14, 17, 23, 21, 23, 16, 14 }, -- "@"
}
local function ascii(): string
	local rows = { "0u", "0u", "0u", "0u", "0u", "0u", "0u" }
	for _, g in RAMP do
		for _, r in g do
			table.insert(rows, `{r}u`)
		end
	end
	return string.format("const FX_RAMP=array<u32,%d>(%s);\nconst FX_N:u32=%du;\n", #rows, table.concat(rows, ","), #RAMP + 1)
		.. [[
const FX_PX:f32=1.1;
const FX_GAMMA:f32=.8;
const FX_T:f32=.1;
const FX_VIVID:f32=.5;
const FX_LIFT:f32=.22;
const FX_K:f32=.34;
const FX_GROUND:f32=.3;
const FX_DIM:f32=.75;
fn fx_a_at(input:LayerInput,p:vec2<f32>)->vec3<f32> {return zgx_sample(clamp(p,vec2(0.),input.size-vec2(.01))).rgb;}
// A font pixel, `FX_PX` points snapped to the device's pixels.
fn fx_px(input:LayerInput)->f32 {let s=max(input.scale,1.); return max(round(FX_PX*s),1.)/s;}
// Font pixel `k`'s colour: its cell's glyph stroke, or the ground.
fn fx_a_pix(input:LayerInput,k:vec2<i32>)->vec3<f32> {
 let light=input.params[0].z>.5;
 let cell=vec2<i32>(floor(vec2<f32>(k)/vec2(6.,8.)));
 let c=fx_a_at(input,(vec2<f32>(cell)+.5)*vec2(6.,8.)*fx_px(input));
 let lum=dot(c,vec3(.2126,.7152,.0722));
 let t=clamp(FX_T+input.params[0].y,0.,.9);
 let level=pow(clamp((select(lum,1.-lum,light)-t)/(1.-t),0.,1.),FX_GAMMA);
 let i=min(u32(level*(f32(FX_N)-.001)),FX_N-1u);
 let at=k-cell*vec2(6,8);
 var lit=false;
 if i>0u && at.x<5 && at.y<7 {lit=((FX_RAMP[i*7u+u32(at.y)]>>u32(4-at.x))&1u)==1u;}
 if lit {
  let peak=max(max(c.r,c.g),max(c.b,.004));
  return mix(mix(c,c/peak,FX_VIVID),select(vec3(1.),vec3(0.),light),FX_LIFT);
 }
 let here=fx_a_at(input,(vec2<f32>(k)+.5)*fx_px(input));
 return mix(fx_paper_srgb(input),here*FX_DIM,FX_GROUND);
}
fn fx_effect(input:LayerInput,g:vec3<f32>,q:vec2<f32>,fp:f32)->vec3<f32> {
 let px=fx_px(input); let x=input.p/px; let w=1./(max(input.scale,1.)*px);
 let k=vec2<i32>(floor(x-.5*w));
 let o=fx_over(x,w);
 var s=fx_a_pix(input,k)*(1.-o.x)*(1.-o.y);
 if o.x>0. {s+=fx_a_pix(input,k+vec2(1,0))*o.x*(1.-o.y);}
 if o.y>0. {s+=fx_a_pix(input,k+vec2(0,1))*(1.-o.x)*o.y;}
 if o.x>0. && o.y>0. {s+=fx_a_pix(input,k+vec2(1,1))*o.x*o.y;}
 return fx_lin(mix(input.color.rgb,s,FX_K));
}
]]
end

-- Halftone: a rounded square in each cell of a screen 7 of the picture's
-- pixels apart, as the cells of Zeron's mark, its area what the colour
-- there takes.
local HALFTONE = [[
const FX_PITCH:f32=7.;
// How far `x` (in cells, from the dot's centre) is outside a rounded
// square covering `k` of its cell.
fn fx_dot(x:vec2<f32>,k:f32)->f32 {
 let r=.5*sqrt(k/.9657);
 let c=.4*r;
 return length(max(abs(x)-vec2(r-c),vec2(0.)))-c;
}
fn fx_effect(input:LayerInput,g:vec3<f32>,q:vec2<f32>,fp:f32)->vec3<f32> {
 let s=q/FX_PITCH;
 let near=floor(s);
 let b=floor(s-.5);
 var ink=FxInk(vec3(0.),vec3(0.));
 var cover=0.;
 for (var j=0.;j<2.;j+=1.) {for (var i=0.;i<2.;i+=1.) {
  let k=b+vec2(i,j);
  let c=fx_mean(input,g,(k+.5)*FX_PITCH,vec2(FX_PITCH*.7));
  let share=fx_share(input,c);
  let d=fx_dot(s-(k+.5),share);
  cover=max(cover,clamp(.5-d*FX_PITCH/fp,0.,1.));
  if all(k==near) {ink=fx_split(input,c,share);}
 }}
 return mix(ink.gap,ink.ink,cover);
}
]]

-- Scanlines: a CRT's rows, 4 of the picture's pixels apart, 55% of each
-- lit, soft at its edges, the dark between lifting where the picture is
-- bright.
local SCANLINES = [[
const FX_PERIOD:f32=4.;
const FX_BAND:f32=.55;
const FX_SOFT:f32=.25;
fn fx_effect(input:LayerInput,g:vec3<f32>,q:vec2<f32>,fp:f32)->vec3<f32> {
 let y=q.y/FX_PERIOD;
 let row=floor(y);
 let c=fx_mean(input,g,vec2(q.x,(row+.5)*FX_PERIOD),vec2(FX_PERIOD*.5,FX_PERIOD));
 let ink=fx_split(input,c,FX_BAND);
 // The band, centred in its row, its edge over the window's pixel (and
 // softened by FX_SOFT of a row).
 let d=(abs(y-row-.5)-.5*FX_BAND)*FX_PERIOD;
 let edge=max(fp,FX_SOFT*FX_PERIOD);
 return mix(ink.gap,ink.ink,clamp(.5-d/edge,0.,1.));
}
]]

local NONE = [[
fn fx_effect(input:LayerInput,g:vec3<f32>,q:vec2<f32>,fp:f32)->vec3<f32> {return fx_lin(input.color.rgb);}
]]

local SOURCES = { dither = DITHER, ascii = ascii(), halftone = HALFTONE, scanlines = SCANLINES } :: { [string]: string }

-- Every effect's layer shader, made as this loads: zgx compiles a shader
-- in the background (70 to 360 ms each), so one made as it's first used
-- left the plain picture up, the effect popping in over it a moment later.
local made: { [string]: any } = { none = ui.layer_shader(COMMON .. NONE) }
for name, source in SOURCES do
	made[name] = ui.layer_shader(COMMON .. source)
end
-- The layer shader for an effect ("dither"; anything else draws none).
function M.shader(name: string): any
	return made[name] or made.none
end

export type Grid = { w: number, h: number, x: number?, y: number?, zoom: number? }
export type Params = {
	strength: number?,
	threshold: number?,
	light: boolean?,
	faded: boolean?,
	picture: Grid?,
}

-- The picture's grid: its pixels as Zeron's proxy of it has them (the
-- picture fitted to 2048 a side, made larger as well as smaller).
function M.grid(p: Grid?): (number, number)
	if not p or p.w <= 0 or p.h <= 0 then
		return 0, 0
	end
	local k = M.GRID_MAX / math.max(p.w, p.h)
	return math.max(1, math.round(p.w * k)), math.max(1, math.round(p.h * k))
end

-- The layer's parameters (`layer_params`): `layerParam0` the strength, 1
-- the density, 3 faded, to set in place.
function M.params(o: Params): { any }
	local p = o.picture
	local gw, gh = M.grid(p)
	local scheme = if o.light then "light" else "dark"
	return {
		{ o.strength or 1, o.threshold or 0, if o.light then 1 else 0, if o.faded == false then 0 else 1 },
		{ gw, gh, if p then math.clamp(p.x or 0.5, 0, 1) else 0.5, if p then math.clamp(p.y or 0.5, 0, 1) else 0.5 },
		{ if p then math.clamp(p.zoom or 1, 1, 4) else 1, M.FLOOR, M.MOST, M.RECEDE[scheme] },
		{ M.PUSH[scheme], 0, 0, 0 },
	}
end

return M
