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

-- The paper, by scheme (sRGB): near black, warm white. Fixed, not the
-- card's fill: a theme's grey card under the ink flattened the dark (the
-- texture a fifth to a half fainter), and every theme draws the same.
M.PAPER = { dark = { 0.043, 0.039, 0.055 }, light = { 0.969, 0.961, 0.945 } }
-- How much of a cell the ink covers, from the paper's own colour to the
-- edge of the colours (the same for every effect): never less than
-- `FLOOR` (the dark of a picture keeps a quiet texture, not a few bright
-- specks), never more than `MOST` (the ink stays a mark on the paper, the
-- paper taking the rest of the colour where it's bright).
M.FLOOR, M.MOST = 0.12, 0.62
-- By scheme: how far past the colour the ink may go from the paper (2.2:
-- more than twice as far), so it reads at a step back yet stays the
-- picture's colour, deeper, not a printer's primary; and how far every
-- effect lets the picture recede into the paper (in sRGB: the room its
-- ink needs over the brightest of a picture in the dark, the darkest in
-- the light): one offset, the same for all.
M.PUSH = { dark = 2.2, light = 1.7 }
M.RECEDE = { dark = 0.07, light = 0.08 }

-- Which candidate each effect draws.
M.VARIANT = { dither = "silk", ascii = "edges", halftone = "tiles", scanlines = "crt" } :: { [string]: string }

local COMMON = string.format(
	[[
// params[0]: strength, density, light (1) or dark (0), faded (1) or not.
// params[1]: the picture's grid (its pixels across, down: 0 for no
//   picture) and where its overflow is cut (0 to 1, 0.5 centred).
// params[2]: its zoom, the ink's least and most share, the recede.
// params[3]: the push, the picture's tones (its lightness's 2nd and 98th
//   percentiles: 0 and 1 where they aren't known).
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
fn fx_split(input:LayerInput,c:vec3<f32>,k:f32)->FxInk {return fx_split_by(input,c,k,input.params[3].x);}
fn fx_split_by(input:LayerInput,c:vec3<f32>,k:f32,push:f32)->FxInk {
 let p=fx_paper(input);
 let ink=p+(c-p)*min(1./max(max(k,fx_reach(c,p)),1e-4),push);
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

-- Dither: every cell of `cell` of the picture's pixels quantized to one of
-- two shades of its own colour, a lighter and a darker, by Bayer's 8 by 8
-- screen: the lighter's share of the cells is the cell's lightness (so a
-- sky's mid tones and a shadow's both take the pattern), and the two shades
-- are `contrast` of as far apart as that share leaves room for (neither
-- past black or white), so the cell's mean is its colour (in sRGB). A
-- gamma `curve` before quantizing sets its brightness against None's.
local function dither(o: { cell: number, contrast: number, curve: number, pull: number }): string
	return cells(o.cell)
		.. string.format("const FX_CONTRAST:f32=%.3f;\nconst FX_CURVE:f32=%.3f;\nconst FX_PULL:f32=%.3f;\n", o.contrast, o.curve, o.pull)
		.. [[
fn fx_threshold(k:vec2<i32>)->f32 {
 let x=u32(k.x); let y=u32(k.y); var v=0u;
 for (var i=0u;i<3u;i++) {v|=(((((x^y)>>i)&1u)<<1u)|((y>>i)&1u))<<(4u-2u*i);}
 return (f32(v)+.5)/64.;
}
fn fx_cell(input:LayerInput,g:vec3<f32>,k:vec2<i32>)->vec3<f32> {
 // The cell's colour, in sRGB, through the curve.
 let c=pow(fx_srgb(fx_mean(input,g,(vec2<f32>(k)+.5)*FX_UNIT,vec2(FX_UNIT))),vec3(FX_CURVE));
 let t=fx_threshold(k);
 // (Pulled towards half by FX_PULL: a shadow takes a fine weave too.)
 let share=clamp(mix(dot(c,vec3(.2126,.7152,.0722)),.5,FX_PULL),.06,.94);
 // As far apart as the colours allow (neither shade past 0 or 1), times
 // the contrast: a shadow takes bright specks, a sky a full weave.
 let room=min((vec3(1.)-c)/(1.-share),c/share);
 let spread=FX_CONTRAST*min(min(room.x,room.y),room.z);
 return fx_lin(select(c-spread*share,c+spread*(1.-share),share>t));
}
]]
end

-- ASCII: the picture made of characters. Cells of 6 by 8 glyph pixels
-- (`px` of the picture's each), each one glyph, its weight by the cell's
-- lightness among the picture's own (stretched over its range, as the
-- host measures it: `image.tones`), so the whole ramp ` .:-=+*#%@` shows
-- in any picture; with `edges`, a contour (Sobel, across the cells round
-- it) takes `- | / \` along it. The glyphs are the light (in the light, the
-- ink): its colour the cell's, quantized lightly, as bright as the cell's
-- mean takes over the picture receded into the paper by `dim` round them.
-- Glyphs, 5 by 7, rows top to bottom (the left pixel the high bit):
-- Zeron's ramp, then the contours.
local GLYPHS: { { number } } = {
	{ 0, 0, 0, 0, 0, 0, 0 }, -- " "
	{ 0, 0, 0, 0, 0, 4, 0 }, -- "."
	{ 0, 4, 0, 0, 4, 0, 0 }, -- ":"
	{ 0, 0, 0, 14, 0, 0, 0 }, -- "-"
	{ 0, 0, 14, 0, 14, 0, 0 }, -- "="
	{ 0, 4, 4, 31, 4, 4, 0 }, -- "+"
	{ 0, 21, 14, 31, 14, 21, 0 }, -- "*"
	{ 10, 10, 31, 10, 31, 10, 10 }, -- "#"
	{ 17, 2, 4, 4, 8, 16, 17 }, -- "%"
	{ 14, 17, 23, 21, 23, 16, 14 }, -- "@"
	{ 0, 0, 0, 31, 0, 0, 0 }, -- "-" (a contour)
	{ 4, 4, 4, 4, 4, 4, 4 }, -- "|"
	{ 1, 2, 2, 4, 8, 8, 16 }, -- "/"
	{ 16, 8, 8, 4, 2, 2, 1 }, -- "\"
}
local RAMP = 10
local function ascii(o: { px: number, edges: boolean, dim: { number }, top: { number }, gamma: { number }, push: number }): string
	local rows, shares = {}, {}
	for _, g in GLYPHS do
		local lit = 0
		for _, r in g do
			table.insert(rows, `{r}u`)
			while r > 0 do
				lit += r % 2
				r = r // 2
			end
		end
		table.insert(shares, string.format("%.5f", lit / 48))
	end
	return string.format(
		[[
const FX_PX:f32=%.4f;
const FX_EDGES:bool=%s;
// By scheme (dark, light): the ground's least and most, the ramp's gamma.
const FX_DIM=vec2<f32>(%.3f,%.3f);
const FX_DIM_TOP=vec2<f32>(%.3f,%.3f);
const FX_GAMMA=vec2<f32>(%.3f,%.3f);
fn fx_by(input:LayerInput,v:vec2<f32>)->f32 {return select(v.x,v.y,input.params[0].z>.5);}
const FX_PUSH:f32=%.3f;
const FX_GLYPHS=array<u32,%d>(%s);
const FX_SHARES=array<f32,%d>(%s);
const FX_RAMP:u32=%du;
const FX_BN=array<u32,1024>(%s);
// A cell's glyph and its ink.
struct FxGlyph { i:u32, ink:vec3<f32>, t:f32 }
fn fx_luma(c:vec3<f32>)->f32 {return dot(fx_srgb(c),vec3(.2126,.7152,.0722));}
// Lightness stretched over the picture's own range (darkness, in the light).
fn fx_rank(input:LayerInput,c:vec3<f32>)->f32 {
 let lo=input.params[3].y; let hi=max(input.params[3].z,lo+.05);
 let s=clamp((fx_luma(c)-lo)/(hi-lo),0.,1.);
 return select(s,1.-s,input.params[0].z>.5);
}
fn fx_glyph(input:LayerInput,g:vec3<f32>,cell:vec2<i32>)->FxGlyph {
 let size=vec2(6.,8.)*FX_PX;
 let at=(vec2<f32>(cell)+.5)*size;
 let c=fx_mean(input,g,at,size);
 let r=pow(fx_rank(input,c),fx_by(input,FX_GAMMA));
 // Between the two glyphs of the ramp round it, by blue noise.
 let n=(u32(cell.y)&63u)*64u+(u32(cell.x)&63u);
 let t=(f32((FX_BN[n>>2u]>>((n&3u)*8u))&255u)+.5)/256.;
 let v=r*f32(FX_RAMP-1u);
 var i=u32(floor(v))+select(0u,1u,fract(v)>t);
 if FX_EDGES && r>.12 {
  // Sobel across the cells round it.
  let l=array<f32,8>(fx_luma(fx_src(input,g,at+size*vec2(-1.,-1.))),fx_luma(fx_src(input,g,at+size*vec2(0.,-1.))),fx_luma(fx_src(input,g,at+size*vec2(1.,-1.))),fx_luma(fx_src(input,g,at+size*vec2(-1.,0.))),fx_luma(fx_src(input,g,at+size*vec2(1.,0.))),fx_luma(fx_src(input,g,at+size*vec2(-1.,1.))),fx_luma(fx_src(input,g,at+size*vec2(0.,1.))),fx_luma(fx_src(input,g,at+size*vec2(1.,1.))));
  let gx=(l[2]+2.*l[4]+l[7])-(l[0]+2.*l[3]+l[5]);
  let gy=(l[5]+2.*l[6]+l[7])-(l[0]+2.*l[1]+l[2]);
  let m=length(vec2(gx,gy))/max(input.params[3].z-input.params[3].y,.05);
  if m>FX_EDGE {
   // The contour runs across the gradient: its angle, in eighths of a turn.
   let a=fract(atan2(gy,gx)/3.14159265+1.);
   let o=u32(floor(a*4.+.5))%%4u;
   i=FX_RAMP+array<u32,4>(1u,2u,0u,3u)[o];
  }
 }
 // Its colour, quantized lightly; the ink as bright as the cell's mean
 // takes over the receded picture round it (no further than the colours go).
 let q=fx_lin(round(fx_srgb(c)*7.)/7.);
 let p=fx_paper(input);
 let k=max(FX_SHARES[i],.02);
 // (Where the receded picture round it stands on the line from the paper.)
 // The glyph as far out from the paper as it goes (no further than
 // FX_PUSH times the colour); the ground round it (`t` of the way from the
 // paper to the colour) what the cell's mean still needs, between FX_DIM
 // and FX_DIM_TOP: the glyphs carry the picture, the ground its glow.
 let f=min(1./max(fx_reach(q,p),1e-4),FX_PUSH);
 let ground=clamp((1.-k*f)/max(1.-k,1e-4),fx_by(input,FX_DIM),fx_by(input,FX_DIM_TOP));
 return FxGlyph(i,p+(q-p)*f,ground);
}
// Whether pixel `at` of a cell (0 to 5 across, 0 to 7 down) is lit.
fn fx_lit(i:u32,at:vec2<i32>)->bool {
 if at.x>4 || at.y>6 {return false;}
 return ((FX_GLYPHS[i*7u+u32(at.y)]>>u32(4-at.x))&1u)==1u;
}
// Glyph pixel `k`'s coverage (its cell's glyph, a neighbour's where it
// reaches into one) and that cell's ink.
fn fx_px(input:LayerInput,g:vec3<f32>,k:vec2<i32>,cell:vec2<i32>,gl:FxGlyph)->vec4<f32> {
 let at=vec2<i32>(floor(vec2<f32>(k)/vec2(6.,8.)));
 var here=gl;
 if any(at!=cell) {here=fx_glyph(input,g,at);}
 return vec4(here.ink,select(0.,1.,fx_lit(here.i,k-at*vec2(6,8))));
}
// The picture receded into the paper round the glyphs, and each glyph
// pixel the window's pixel covers, by how much falls there.
fn fx_effect(input:LayerInput,g:vec3<f32>,q:vec2<f32>,fp:f32)->vec3<f32> {
 let x=q/FX_PX; let w=fp/FX_PX;
 let k=vec2<i32>(floor(x-.5*w));
 let o=fx_over(x,w);
 let cell=vec2<i32>(floor(floor(x)/vec2(6.,8.)));
 let gl=fx_glyph(input,g,cell);
 // The ground: the picture blurred over a cell (its glow, none of its
 // detail), `t` of the way from the paper.
 let p=fx_paper(input);
 let s=p+(fx_mean(input,g,q,vec2(6.,8.)*FX_PX)-p)*gl.t;
 var ink=vec3(0.); var cover=0.;
 for (var j=0;j<2;j++) {for (var i=0;i<2;i++) {
  let wt=select(1.-o.x,o.x,i==1)*select(1.-o.y,o.y,j==1);
  if wt>0. {let e=fx_px(input,g,k+vec2(i,j),cell,gl); ink+=e.rgb*e.a*wt; cover+=e.a*wt;}
 }}
 return s*(1.-cover)+ink;
}
]],
		o.px,
		if o.edges then "true" else "false",
		o.dim[1],
		o.dim[2],
		o.top[1],
		o.top[2],
		o.gamma[1],
		o.gamma[2],
		o.push,
		#rows,
		table.concat(rows, ","),
		#shares,
		table.concat(shares, ","),
		RAMP,
		blue_noise
	) .. "const FX_EDGE:f32=2.0;\n"
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
		-- Two shades of each cell's colour, cells of 2 points.
		silk = function()
			return dither { cell = 2.5, contrast = 0.55, curve = 1.06, pull = 0.3 }
		end,
		-- The same, finer: cells of 1.5 points.
		fine = function()
			return dither { cell = 1.9, contrast = 0.55, curve = 1.06, pull = 0.3 }
		end,
		-- The shades further apart: closer to Zeron's lit-or-dim cells.
		bold = function()
			return dither { cell = 2.5, contrast = 0.85, curve = 1.06, pull = 0.15 }
		end,
	},
	ascii = {
		-- Cells of 6 by 8 points or so, contours drawn.
		edges = function()
			return ascii { px = 1.25, edges = true, dim = { 0.1, 0.3 }, top = { 0.55, 0.85 }, gamma = { 0.8, 0.6 }, push = 3 }
		end,
		-- The same without contours.
		plain = function()
			return ascii { px = 1.25, edges = false, dim = { 0.1, 0.3 }, top = { 0.55, 0.85 }, gamma = { 0.8, 0.6 }, push = 3 }
		end,
		-- Larger cells, contours drawn.
		large = function()
			return ascii { px = 1.6, edges = true, dim = { 0.1, 0.3 }, top = { 0.55, 0.85 }, gamma = { 0.8, 0.6 }, push = 3 }
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
		-- A CRT's rows, 4 of the picture's pixels apart: 55% of each lit, soft.
		crt = function()
			return scanlines { period = 4, band = 0.55, soft = 0.25 }
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
	-- The picture's tones (`image.tones`: its lightness's 2nd and 98th
	-- percentiles), where known.
	tones: { lo: number, hi: number }?,
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
-- the density, 3 faded, 13 and 14 the tones, to set in place.
function M.params(o: Params): { any }
	local p = o.picture
	local gw, gh = M.grid(p)
	local scheme = if o.light then "light" else "dark"
	local tones = o.tones or { lo = 0, hi = 1 }
	return {
		{ o.strength or 1, o.threshold or 0, if o.light then 1 else 0, if o.faded == false then 0 else 1 },
		{ gw, gh, if p then math.clamp(p.x or 0.5, 0, 1) else 0.5, if p then math.clamp(p.y or 0.5, 0, 1) else 0.5 },
		{ if p then p.zoom or 1 else 1, M.FLOOR, M.MOST, M.RECEDE[scheme] },
		{ M.PUSH[scheme], tones.lo, tones.hi, 0 },
	}
end

return M
