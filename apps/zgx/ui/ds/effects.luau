-- A new chat's wallpaper effects: each a layer shader over the whole
-- picture as it's composited, so the art in it (a picture, or what a
-- plugin draws) takes one pass, not one a layer.
--
-- `ui.div { layer_shader = effects.shader("dither"), layer_params = effects.params { … }, … }`
--
-- One tone for every effect. Each works in linear light on the picture's
-- own grid (the picture fitted to 2048 a side, as Zeron's proxy of it, so
-- the cells hold to the picture and grow and shrink with it), and prints
-- each cell with ink on paper so that, seen from a step back, the cell is
-- the picture's own colour there:
--
-- 1. the picture recedes into the paper by one small offset (`RECEDE`);
-- 2. a cell's ink covers a share of it from `FLOOR` to `MOST`, by how far
--    its colour stands from the paper (`fx_share`);
-- 3. the ink lies on the line from the paper through that colour, further
--    out (no more than `PUSH`), and the gaps between make up the rest, so
--    the cell's mean is the colour's exactly (`fx_split`).
--
-- Switching effects changes the texture, never how bright or how coloured
-- the picture is. The paper follows the scheme (near black in the dark,
-- warm white in the light), the same rule for every effect. Every edge is
-- worked out over the window's own pixel (crisp at 2x, no shimmer), and
-- every effect, None too, fades the art from its top, full, to nothing at
-- its foot.
--
-- Each effect has a few candidates (`M.VARIANTS`); `M.VARIANT` is the one
-- the app draws, `"dither/silk"` names another.

local blue_noise = require("@self/blue_noise")

local M = {}

-- The longer side of the picture's grid (Zeron's proxy of the picture).
M.GRID_MAX = 2048

-- The paper, by scheme (sRGB).
M.PAPER = { dark = { 0.043, 0.039, 0.055 }, light = { 0.969, 0.961, 0.945 } }
-- How much of a cell the ink covers, from the paper's own colour to the
-- edge of the colours (the same for every effect): never less than
-- `FLOOR` (the dark of a picture keeps a quiet texture, not a few bright
-- specks), never more than `MOST` (the ink stays a mark on a ground, the
-- ground taking the rest of the colour where it's bright).
M.FLOOR, M.MOST = 0.12, 0.72
-- By scheme: how far past the colour the ink may go from the paper (1.6:
-- six tenths further), so its colour stays the picture's, deeper, not a
-- printer's primary; and how far every effect lets the picture recede into
-- the paper (the room its ink needs over the brightest of a picture in the
-- dark, the darkest in the light): one small offset, the same for all. The
-- light's are less, as ink on white reads stronger than light on black.
M.PUSH = { dark = 1.6, light = 1.35 }
M.RECEDE = { dark = 0.05, light = 0.04 }

-- Which candidate each effect draws.
M.VARIANT = { dither = "film", ascii = "large", halftone = "round", scanlines = "crt" } :: { [string]: string }

local COMMON = [[
// params[0]: strength, density, light (1) or dark (0), faded (1) or not.
// params[1]: the picture's grid (its pixels across, down: 0 for no
//   picture) and where its overflow is cut (0 to 1, 0.5 centred).
// params[2]: its zoom, the ink's least and most share, the recede.
// params[3]: the paper (sRGB), the push.
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
 return fx_lin(mix(c,input.params[3].rgb,input.params[2].w));
}
// Its mean over a box `r` of its pixels across, centred at `q`.
fn fx_mean(input:LayerInput,g:vec3<f32>,q:vec2<f32>,r:vec2<f32>)->vec3<f32> {
 let d=r*.25;
 return .25*(fx_src(input,g,q-d)+fx_src(input,g,q+d)+fx_src(input,g,q+vec2(d.x,-d.y))+fx_src(input,g,q+vec2(-d.x,d.y)));
}
fn fx_paper(input:LayerInput)->vec3<f32> {return fx_lin(input.params[3].rgb);}
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
 let ink=p+(c-p)*min(1./max(max(k,fx_reach(c,p)),1e-4),input.params[3].w);
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
]]

-- A cell-by-cell effect (`fx_cell(input, g, k)`, cells `unit` of the
-- picture's pixels a side): the window's pixel takes each cell it covers
-- by how much of it falls there.
local function cells(unit: number): string
	return string.format(
		[[
const FX_UNIT:f32=%.4f;
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
]],
		unit
	)
end

-- Dither: each cell of `cell` pixels ink or gap (or, with `levels` past
-- 2, a step between), by a threshold map: blue noise (a grain with no
-- pattern) or Bayer's 8 by 8 (an even, woven screen).
local function dither(o: { cell: number, levels: number, map: string }): string
	return cells(o.cell)
		.. string.format("const FX_LEVELS:f32=%.1f;\nconst FX_BN=array<u32,1024>(%s);\n", o.levels, blue_noise)
		.. (if o.map == "bayer"
			then [[
fn fx_threshold(k:vec2<i32>)->f32 {
 let x=u32(k.x); let y=u32(k.y); var v=0u;
 for (var i=0u;i<3u;i++) {v|=(((((x^y)>>i)&1u)<<1u)|((y>>i)&1u))<<(4u-2u*i);}
 return (f32(v)+.5)/64.;
}
]]
			else [[
fn fx_threshold(k:vec2<i32>)->f32 {
 let i=(u32(k.y)&63u)*64u+(u32(k.x)&63u);
 return (f32((FX_BN[i>>2u]>>((i&3u)*8u))&255u)+.5)/256.;
}
]])
		.. [[
fn fx_cell(input:LayerInput,g:vec3<f32>,k:vec2<i32>)->vec3<f32> {
 let c=fx_mean(input,g,(vec2<f32>(k)+.5)*FX_UNIT,vec2(FX_UNIT));
 let s=fx_share(input,c);
 let ink=fx_split(input,c,s);
 // The steps between the gap and the ink, one taken by the threshold.
 let v=s*(FX_LEVELS-1.);
 let step=floor(v)+select(0.,1.,fract(v)>fx_threshold(k));
 return mix(ink.gap,ink.ink,step/(FX_LEVELS-1.));
}
]]
end

-- ASCII: cells of 6 by 8 glyph pixels (`px` of the picture's each), each
-- the glyph whose strokes cover what the colour there takes.
-- Glyphs, 5 by 7, rows top to bottom (the left pixel the high bit).
local GLYPHS: { { number } } = {
	{ 0, 0, 0, 0, 0, 0, 0 }, -- " "
	{ 0, 0, 0, 0, 0, 12, 12 }, -- "."
	{ 0, 0, 0, 31, 0, 0, 0 }, -- "-"
	{ 0, 12, 12, 0, 12, 12, 0 }, -- ":"
	{ 0, 4, 4, 31, 4, 4, 0 }, -- "+"
	{ 0, 0, 31, 0, 31, 0, 0 }, -- "="
	{ 0, 4, 21, 14, 21, 4, 0 }, -- "*"
	{ 0, 0, 14, 17, 17, 17, 14 }, -- "o"
	{ 24, 25, 2, 4, 8, 19, 3 }, -- "%"
	{ 12, 18, 20, 8, 21, 18, 13 }, -- "&"
	{ 10, 10, 31, 10, 31, 10, 10 }, -- "#"
	{ 14, 17, 23, 21, 23, 16, 15 }, -- "@"
	{ 30, 17, 17, 30, 17, 17, 30 }, -- "B"
}
local function ascii(o: { px: number, dots: boolean, sparse: number }): string
	local rows, shares = {}, {}
	for _, glyph in GLYPHS do
		local lit = 0
		for _, row in glyph do
			table.insert(rows, `{row}u`)
			local r = row
			while r > 0 do
				lit += r % 2
				r = r // 2
			end
		end
		table.insert(shares, string.format("%.5f", lit / 48 * (if o.dots then math.pi * 0.42 * 0.42 else 1)))
	end
	return string.format(
		[[
const FX_PX:f32=%.4f;
const FX_SPARSE:f32=%.3f;
const FX_GLYPHS=array<u32,%d>(%s);
const FX_SHARES=array<f32,%d>(%s);
const FX_COUNT:u32=%du;
// The cell's glyph and its ink (the glyph whose share of the cell is
// enough, the density moving it lighter or heavier).
struct FxGlyph { i:u32, ink:FxInk, c:vec3<f32> }
fn fx_glyph(input:LayerInput,g:vec3<f32>,cell:vec2<i32>)->FxGlyph {
 let size=vec2(6.,8.)*FX_PX;
 let c=fx_mean(input,g,(vec2<f32>(cell)+.5)*size,size);
 // (Its share of the most a glyph covers.)
 let want=fx_share(input,c)/input.params[2].z*FX_SHARES[FX_COUNT-1u]*FX_SPARSE*(1.-input.params[0].y);
 var i=0u;
 loop {if i+1u>=FX_COUNT || FX_SHARES[i]>=want {break;} i++;}
 return FxGlyph(i,fx_split(input,c,FX_SHARES[i]),c);
}
// The glyph's ink (`cover` of the window's pixel) and its gap at `q`, each
// as the cell's are to its colour, over the picture there: the strokes
// keep the picture's own detail and stay lighter (darker, in the light)
// than the gap however the picture runs under them.
fn fx_glyph_at(input:LayerInput,g:vec3<f32>,q:vec2<f32>,gl:FxGlyph,cover:f32)->vec3<f32> {
 let c=max(gl.c,vec3(1e-4));
 return clamp(fx_src(input,g,q)*mix(gl.ink.gap,gl.ink.ink,cover)/c,vec3(0.),vec3(1.));
}
// Whether glyph pixel `x`, `y` of glyph `i` is lit.
fn fx_lit(i:u32,x:i32,y:i32)->bool {
 if x>4 || y>6 {return false;}
 return ((FX_GLYPHS[i*7u+u32(y)]>>u32(4-x))&1u)==1u;
}
]],
		o.px,
		o.sparse,
		#rows,
		table.concat(rows, ","),
		#shares,
		table.concat(shares, ","),
		#shares
	) .. (if o.dots
		then [[
// Each lit pixel a round dot (a dot matrix): the four glyph pixels round
// the window's, each its dot's coverage.
fn fx_effect(input:LayerInput,g:vec3<f32>,q:vec2<f32>,fp:f32)->vec3<f32> {
 let x=q/FX_PX;
 let here=vec2<i32>(floor(x));
 let cell=vec2<i32>(floor(vec2<f32>(here)/vec2(6.,8.)));
 let gl=fx_glyph(input,g,cell);
 let b=vec2<i32>(floor(x-.5));
 var cover=0.;
 for (var j=0;j<2;j++) {for (var i=0;i<2;i++) {
  let k=b+vec2(i,j);
  let at=k-cell*vec2(6,8);
  if all(at>=vec2(0)) && all(at<vec2(6,8)) && fx_lit(gl.i,at.x,at.y) {
   let d=length(x-(vec2<f32>(k)+.5))-.42;
   cover=max(cover,clamp(.5-d*FX_PX/fp,0.,1.));
  }
 }}
 return fx_glyph_at(input,g,q,gl,cover);
}
]]
		else [[
// Each lit pixel a square: the window's pixel takes each glyph pixel it
// covers by how much falls there (the cell's glyph, its own edge a gap).
fn fx_effect(input:LayerInput,g:vec3<f32>,q:vec2<f32>,fp:f32)->vec3<f32> {
 let x=q/FX_PX; let w=fp/FX_PX;
 let k=vec2<i32>(floor(x-.5*w));
 let o=fx_over(x,w);
 let cell=vec2<i32>(floor(floor(x)/vec2(6.,8.)));
 let gl=fx_glyph(input,g,cell);
 var s=0.;
 for (var j=0;j<2;j++) {for (var i=0;i<2;i++) {
  let at=k+vec2(i,j)-cell*vec2(6,8);
  let wt=select(1.-o.x,o.x,i==1)*select(1.-o.y,o.y,j==1);
  if wt>0. && all(at>=vec2(0)) && fx_lit(gl.i,at.x,at.y) {s+=wt;}
 }}
 return fx_glyph_at(input,g,q,gl,s);
}
]])
end

-- Halftone: a dot in each cell of a screen `pitch` pixels apart at `angle`
-- degrees, its area what the colour there takes: round (growing into a
-- chain, then leaving holes) or a rounded square, as Zeron's mark's cells.
local function halftone(o: { pitch: number, angle: number, square: boolean }): string
	local a = math.rad(o.angle)
	return string.format(
		[[
const FX_PITCH:f32=%.4f;
const FX_ROT=mat2x2<f32>(%.6f,%.6f,%.6f,%.6f);
const FX_SQUARE:bool=%s;
// A dot's size for a share `k` of its cell (in cells): a round dot's
// radius (clipped by the cell past a half), a square's half side.
fn fx_size(k:f32)->f32 {
 if FX_SQUARE {return .5*sqrt(k/.9657);}
 if k<=.785398 {return sqrt(k/3.14159265);}
 var r=mix(.5,.70711,(k-.785398)/.214602);
 for (var n=0;n<4;n++) {
  let a=acos(min(.5/r,1.));
  let area=3.14159265*r*r-4.*(r*r*a-.5*sqrt(max(r*r-.25,0.)));
  r=clamp(r-(area-k)/max(2.*r*(3.14159265-4.*a),1e-3),.5,.70711);
 }
 return r;
}
// How far `x` (in cells, from the dot's centre) is outside a dot of size `r`.
fn fx_dot(x:vec2<f32>,r:f32)->f32 {
 if FX_SQUARE {let c=.4*r; return length(max(abs(x)-vec2(r-c),vec2(0.)))-c;}
 return length(x)-r;
}
fn fx_effect(input:LayerInput,g:vec3<f32>,q:vec2<f32>,fp:f32)->vec3<f32> {
 let s=FX_ROT*q/FX_PITCH;
 let near=floor(s);
 let b=floor(s-.5);
 var ink=FxInk(vec3(0.),vec3(0.));
 var cover=0.;
 for (var j=0.;j<2.;j+=1.) {for (var i=0.;i<2.;i+=1.) {
  let k=b+vec2(i,j);
  let c=fx_mean(input,g,transpose(FX_ROT)*((k+.5)*FX_PITCH),vec2(FX_PITCH*.7));
  let share=fx_share(input,c);
  let d=fx_dot(s-(k+.5),fx_size(share));
  cover=max(cover,clamp(.5-d*FX_PITCH/fp,0.,1.));
  if all(k==near) {ink=fx_split(input,c,share);}
 }}
 return mix(ink.gap,ink.ink,cover);
}
]],
		o.pitch,
		math.cos(a),
		math.sin(a),
		-math.sin(a),
		math.cos(a),
		if o.square then "true" else "false"
	)
end

-- Scanlines: rows `period` pixels apart, each a band of ink across the
-- paper: a CRT's (a fixed `band` of each row lit, soft at its edges, the
-- dark between lifting where the picture is bright) or a line screen's (the
-- band as thick as the colour there takes, sharp).
local function scanlines(o: { period: number, band: number?, soft: number }): string
	return string.format(
		[[
const FX_PERIOD:f32=%.4f;
const FX_BAND:f32=%.4f;
const FX_SOFT:f32=%.4f;
fn fx_effect(input:LayerInput,g:vec3<f32>,q:vec2<f32>,fp:f32)->vec3<f32> {
 let y=q.y/FX_PERIOD;
 let row=floor(y);
 let c=fx_mean(input,g,vec2(q.x,(row+.5)*FX_PERIOD),vec2(FX_PERIOD*.5,FX_PERIOD));
 let share=select(fx_share(input,c),FX_BAND,FX_BAND>0.);
 let ink=fx_split(input,c,share);
 // The band, centred in its row, its edge over the window's pixel (and
 // softened by FX_SOFT of a row).
 let d=(abs(y-row-.5)-.5*share)*FX_PERIOD;
 let edge=max(fp,FX_SOFT*FX_PERIOD);
 return mix(ink.gap,ink.ink,clamp(.5-d/edge,0.,1.));
}
]],
		o.period,
		o.band or 0,
		o.soft
	)
end

-- Every candidate, by effect: what each draws.
M.VARIANTS = {
	dither = {
		-- Blue noise, ink or gap: a stipple grain with no pattern.
		grain = function()
			return dither { cell = 1.5, levels = 2, map = "blue" }
		end,
		-- Bayer's 8 by 8, ink or gap: an even, woven screen.
		silk = function()
			return dither { cell = 2, levels = 2, map = "bayer" }
		end,
		-- Blue noise, three steps: a soft film grain.
		film = function()
			return dither { cell = 2, levels = 3, map = "blue" }
		end,
	},
	ascii = {
		-- Square pixels, cells of 6 by 8.
		mono = function()
			return ascii { px = 1.25, dots = false, sparse = 0.6 }
		end,
		-- A dot matrix: each lit pixel a round dot.
		matrix = function()
			return ascii { px = 1.25, dots = true, sparse = 0.6 }
		end,
		-- Larger glyphs.
		large = function()
			return ascii { px = 1.75, dots = false, sparse = 0.6 }
		end,
	},
	halftone = {
		-- Round dots at 45°, the press's screen.
		round = function()
			return halftone { pitch = 5, angle = 45, square = false }
		end,
		-- Rounded squares on the grid, as the cells of Zeron's mark.
		tiles = function()
			return halftone { pitch = 7, angle = 0, square = true }
		end,
		-- Finer round dots at 15°.
		fine = function()
			return halftone { pitch = 3.5, angle = 15, square = false }
		end,
	},
	scanlines = {
		-- A CRT's rows: 60% of each lit, soft.
		crt = function()
			return scanlines { period = 3, band = 0.62, soft = 0.4 }
		end,
		-- A line screen: each row's band as thick as its colour takes.
		lines = function()
			return scanlines { period = 3.5, soft = 0 }
		end,
		-- Fine CRT rows, sharper.
		fine = function()
			return scanlines { period = 2, band = 0.55, soft = 0.1 }
		end,
	},
} :: { [string]: { [string]: () -> string } }

local NONE = [[
fn fx_effect(input:LayerInput,g:vec3<f32>,q:vec2<f32>,fp:f32)->vec3<f32> {return fx_lin(input.color.rgb);}
]]

local made: { [string]: any } = {}
-- The layer shader for an effect ("dither", or a candidate: "dither/silk"),
-- made once.
function M.shader(name: string): any
	local e, v = string.match(name, "^([^/]*)/?(.*)$")
	local effect: string, variant: string = e or "none", v or ""
	local of = M.VARIANTS[effect]
	if not of then
		effect, variant = "none", ""
	elseif variant == "" or not of[variant] then
		variant = M.VARIANT[effect] or ""
	end
	local key = `{effect}/{variant}`
	if not made[key] then
		local make = if of then of[variant] else nil
		made[key] = ui.layer_shader(COMMON .. (if make then make() else NONE))
	end
	return made[key]
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
-- the density and 3 faded, to set in place.
function M.params(o: Params): { any }
	local p = o.picture
	local gw, gh = M.grid(p)
	local scheme = if o.light then "light" else "dark"
	local paper = M.PAPER[scheme]
	return {
		{ o.strength or 1, o.threshold or 0, if o.light then 1 else 0, if o.faded == false then 0 else 1 },
		{ gw, gh, if p then math.clamp(p.x or 0.5, 0, 1) else 0.5, if p then math.clamp(p.y or 0.5, 0, 1) else 0.5 },
		{ if p then p.zoom or 1 else 1, M.FLOOR, M.MOST, M.RECEDE[scheme] },
		{ paper[1], paper[2], paper[3], M.PUSH[scheme] },
	}
end

return M
