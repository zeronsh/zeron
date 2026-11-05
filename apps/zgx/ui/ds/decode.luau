-- Screens decoding out of characters, and dissolving back into them: the
-- intro scene's own (zgx's `characters`, its 5 by 7 font of ` .-+xXS80#@`
-- and its reveal, from zgx-gpu's image_effects.wgsl, MIT), as layer
-- shaders over any subtree. Two fronts share one look:
--
-- `decode.shader` opens from a point: `layer_shader = decode.shader,
-- layer_params = decode.params`, then animate `layerParam0` from 0 to
-- `decode.FULL`. `layerParam0` to `3`: front, band, ragged, a font pixel in
-- points; `4`, `5`: where it opens, in points from the middle. The front is
-- measured out to a rounded box at 1 (the subtree's half-extents).
--
-- `decode.collapse` closes onto a rounded rect: everything farther from it
-- than the front is gone. `layerParam0` to `3` as above, front and band in
-- points; `4` to `7`: the rect (x, y, w, h, points in the subtree); `8`: its
-- corner radius. Animate `layerParam0` from `decode.reach(rect, w, h)` down
-- to 0, and what's left is the rect's own shape, its edge in characters.
--
-- Either way: ahead of the front, nothing; across its band, characters
-- scrambling; behind it the content fades in under its glyphs, and further
-- behind the glyphs go, leaving the content exactly as drawn.

-- The font and the cells' own random numbers, shared with every shader that
-- draws characters (the member card's sky too).
local GLYPHS = [[
fn glyph(i:u32)->vec2<u32> {
 switch i {
  case 1u: {return vec2(402653184u,12u);} case 2u: {return vec2(1015808u,0u);}
  case 3u: {return vec2(139432064u,0u);} case 4u: {return vec2(340083712u,17u);}
  case 5u: {return vec2(581052977u,17u);} case 6u: {return vec2(35078671u,30u);}
  case 7u: {return vec2(588727854u,14u);} case 8u: {return vec2(597347886u,14u);}
  case 9u: {return vec2(368409930u,10u);} case 10u: {return vec2(561700398u,15u);}
  default: {return vec2(0u);}
 }
}
fn cell_hash(c:vec2<u32>)->f32 {
 var h=c.x*1664525u+c.y*1013904223u+0x9e3779b9u;
 h^=h>>16u; h*=2246822519u; h^=h>>13u; h*=3266489917u; h^=h>>16u;
 return f32(h&16777215u)/16777216.;
}
]]

local COMMON = GLYPHS .. [[
// The pixel, `into` bands behind the front at its cell (ragged) and
// `behind` bands at the pixel itself.
fn decoded(input:LayerInput,cell:vec2<f32>,cell_size:vec2<f32>,px:f32,into:f32,behind:f32)->vec4<f32> {
 let at=zgx_sample((cell+.5)*cell_size);
 let own=input.color;
 let c=vec2<u32>(max(cell,vec2(0.)));
 let shown=smoothstep(.35,1.35,behind);
 let clean=smoothstep(1.2,2.4,behind);
 let glyph_alpha=select(0.,smoothstep(0.,.6,into),cell_hash(c+vec2(77u,13u))<pow(clamp(into,0.,1.),.7))
  *(1.-smoothstep(1.2,2.4,into));
 var index=u32(pow(clamp((dot(at.rgb,vec3(.299,.587,.114))*at.a-.3)/.65,0.,1.),1.6)*10.999);
 if into<1. {let s=u32(max(into,0.)*12.); index=1u+u32(cell_hash(c+vec2(s*7u+1u,s))*9.999);}
 let g=glyph(index);
 let fp=vec2<u32>(floor((input.p-cell*cell_size)/px));
 var lit=0.;
 if fp.x<5u && fp.y<7u {lit=f32((select(g.y,g.x>>(fp.y*5u),fp.y<6u)>>(4u-fp.x))&1u);}
 // Lit in the content's colour at full value; over dark glass, the scene's
 // pale lilac.
 let peak=max(at.r,max(at.g,at.b));
 let ink=mix(vec3(.80,.78,1.),at.rgb/max(peak,.001),smoothstep(.15,.5,peak));
 let k=lit*glyph_alpha*mix(1.,.65,shown)*select(0.,1.,at.a>.01);
 let pa=own.a*shown;
 let pr=own.rgb*mix(.88,1.,clean);
 let a=k+pa*(1.-k);
 return vec4((ink*k+pr*pa*(1.-k))/max(a,.0001),a);
}
]]

local shader = ui.layer_shader(COMMON .. [[
fn layer(input:LayerInput)->vec4<f32> {
 let r=input.params[0];
 let px=max(r.w,1.);
 let cell_size=vec2(6.,8.)*px;
 let cell=floor(input.p/cell_size);
 let middle=(cell+.5)*cell_size;
 let half=input.size*.5;
 let origin=half+input.params[1].xy;
 let q=abs(input.p-origin)/half; let q4=q*q*q*q;
 let qc=abs(middle-origin)/half; let qc4=qc*qc*qc*qc;
 let band=max(r.y,.001);
 let c=vec2<u32>(max(cell,vec2(0.)));
 let into=(r.x-sqrt(sqrt(qc4.x+qc4.y))+(cell_hash(c)-.5)*r.z)/band;
 let behind=(r.x-sqrt(sqrt(q4.x+q4.y)))/band;
 return decoded(input,cell,cell_size,px,into,behind);
}]])

local collapse = ui.layer_shader(COMMON .. [[
// Signed distance from a rounded rect (its middle at 0): negative inside.
fn rounded_rect(p:vec2<f32>,half:vec2<f32>,r:f32)->f32 {
 let q=abs(p)-half+vec2(r);
 return length(max(q,vec2(0.)))+min(max(q.x,q.y),0.)-r;
}
fn layer(input:LayerInput)->vec4<f32> {
 let r=input.params[0];
 let rect=input.params[1];
 let radius=input.params[2].x;
 let px=max(r.w,1.);
 let cell_size=vec2(6.,8.)*px;
 let cell=floor(input.p/cell_size);
 let middle=(cell+.5)*cell_size;
 let half=rect.zw*.5;
 let centre=rect.xy+half;
 let band=max(r.y,1.);
 let c=vec2<u32>(max(cell,vec2(0.)));
 let into=(r.x-rounded_rect(middle-centre,half,radius)+(cell_hash(c)-.5)*r.z*band)/band;
 let behind=(r.x-rounded_rect(input.p-centre,half,radius))/band;
 return decoded(input,cell,cell_size,px,into,behind);
}]])

-- A collapse's band, in points.
local COLLAPSE_BAND = 120

-- The front a collapse onto `rect` starts from, in a `w` by `h` subtree:
-- past its farthest corner and the band behind the front, so all of it
-- shows as drawn.
local function reach(rect: { x: number, y: number, w: number, h: number }, w: number, h: number): number
	local dx = math.max(rect.x, w - rect.x - rect.w)
	local dy = math.max(rect.y, h - rect.y - rect.h)
	return math.sqrt(dx * dx + dy * dy) + COLLAPSE_BAND * 2.4
end

-- The front a `decode.shader` must reach for the screen to be whole when it
-- opens from (`ox`, `oy`), points from the middle of a `w` by `h` subtree:
-- past its farthest corner (in the shader's own measure) and the band behind
-- the front. `FULL` is this from the middle.
local function full(ox: number, oy: number, w: number, h: number): number
	local hw, hh = w / 2, h / 2
	local far = 0
	for _, x in { -hw, hw } do
		for _, y in { -hh, hh } do
			local qx, qy = math.abs(x - ox) / hw, math.abs(y - oy) / hh
			far = math.max(far, (qx ^ 4 + qy ^ 4) ^ 0.25)
		end
	end
	return far + 0.45 * 2.4
end

return {
	-- WGSL: `glyph(i)` (` .-+xXS80#@`, 5 by 7) and `cell_hash(c)`.
	GLYPHS = GLYPHS,
	shader = shader,
	-- Nothing shown; a band of 0.45, ragged 0.14, a font pixel of 2 points
	-- (the scene's), opening from the middle.
	params = { { 0, 0.45, 0.14, 2 }, { 0, 0, 0, 0 } },
	-- `layerParam0` with the screen whole: past every corner and its band.
	FULL = 2.4,
	full = full,

	collapse = collapse,
	-- Everything shown (a front past any screen), the band, ragged 0.5 of
	-- it, the same font pixel; no rect yet.
	collapse_params = { { 100000, COLLAPSE_BAND, 0.5, 2 }, { 0, 0, 0, 0 }, { 0, 0, 0, 0 } },
	reach = reach,
}
