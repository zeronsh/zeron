-- A new chat's wallpaper effects, as Zeron's draws them (crates/ui/src/
-- new_thread_background_effects.rs): each a layer shader over the whole
-- picture as it's composited, so the art in it (a picture, or what a
-- plugin draws) takes one pass, not one a layer.
--
-- `ui.div { layer_shader = effects.SHADERS.dither, layer_params = effects.params { … }, … }`
--
-- Zeron drew its effects on the CPU into the picture's own pixels (the
-- picture fitted to 2048 a side first), then laid that picture over the
-- window, scaled to cover, filtered as a picture is. These do the same on
-- the GPU: the effect is worked out for each of the picture's pixels, on
-- its grid (so it holds to the picture and grows and shrinks with it, the
-- dots of a halftone a few of its pixels apart at any window size), and
-- the window's pixel between four of them takes their blend, as a
-- picture's filtering gives. The treatments are Zeron's:
--
-- * Dither: a 4×4 Bayer screen in cells of 2 by 2 of the picture's pixels,
--   each lit in its colour at full value where its brightest channel
--   passes the cell's threshold, else that colour at 8%.
-- * ASCII: cells of 6 by 8 pixels, each the glyph of ` .:-=+*#%@` whose
--   ink is the brightness there (in the light, the darkness), its strokes
--   in the cell's colour, over the picture at 60% on its paper (black in
--   the dark, white in the light).
-- * Halftone: a dot in each 4 by 4 cell, its radius by the brightness (the
--   darkness in the light), in the cell's colour on its paper, over the
--   picture at 60%.
-- * Scanlines: every third row of the picture's pixels darkened to 52% (in
--   the light, lifted that much towards white).
--
-- Every effect, None too, draws the art faded from its top, full, to
-- nothing at its foot (a smoothstep over the box's height, as zgx's edge
-- fade): in the shader, so it follows the box with no script and no mask.

local M = {}

-- The longer side of the picture's grid (Zeron's proxy of the picture).
M.GRID_MAX = 2048

local COMMON = [[
// params[0]: strength, threshold, light (1) or dark (0), faded (1) or not.
// params[1]: the picture's grid (its pixels across, down: 0 for no
//   picture) and where its overflow is cut (0 to 1, 0.5 centred).
// params[2]: its zoom.
// The picture's grid, covering the box: its top left in points, and a
// pixel's size (half a point without a picture).
fn fx_grid(input:LayerInput)->vec3<f32> {
 let pw=input.params[1].x; let ph=input.params[1].y;
 if pw<=0. || ph<=0. {return vec3(0.,0.,.5);}
 let s=max(input.size.x/pw,input.size.y/ph)*max(input.params[2].x,1.);
 return vec3(input.params[1].z*(input.size.x-pw*s),input.params[1].w*(input.size.y-ph*s),s);
}
// The picture's pixel `i` (whole, from its top left).
fn fx_src(input:LayerInput,g:vec3<f32>,i:vec2<u32>)->vec3<f32> {
 return zgx_sample(g.xy+(vec2<f32>(i)+.5)*g.z).rgb;
}
fn fx_luma(c:vec3<f32>)->f32 {return dot(c,vec3(.2126,.7152,.0722));}
fn fx_light(input:LayerInput)->bool {return input.params[0].z>.5;}
fn fx_paper(input:LayerInput)->vec3<f32> {return select(vec3(0.),vec3(1.),fx_light(input));}
// How much of the art shows here: full at the top, nothing at the foot.
fn fx_alpha(input:LayerInput)->f32 {
 if input.params[0].w<.5 {return 1.;}
 return smoothstep(0.,input.size.y,input.size.y-input.p.y);
}
]]

local function shader(effect: string): any
	return ui.layer_shader(COMMON .. effect .. [[
// The effect at the window's pixel: worked out at the four of the
// picture's pixels round it, blended as a picture's filtering blends them.
fn layer(input:LayerInput)->vec4<f32> {
 let g=fx_grid(input);
 let u=(input.p-g.xy)/g.z-.5;
 let f=fract(u);
 let i0=vec2<u32>(max(vec2<i32>(floor(u)),vec2(0)));
 let e=mix(mix(fx_effect(input,g,i0),fx_effect(input,g,i0+vec2(1u,0u)),f.x),
  mix(fx_effect(input,g,i0+vec2(0u,1u)),fx_effect(input,g,i0+vec2(1u,1u)),f.x),f.y);
 let rgb=clamp(mix(input.color.rgb,e,input.params[0].x),vec3(0.),vec3(1.));
 return vec4(rgb,input.color.a*fx_alpha(input));
}
]])
end

M.SHADERS = ({
	-- The picture as it is, faded.
	none = ui.layer_shader(COMMON .. [[
fn layer(input:LayerInput)->vec4<f32> {
 return vec4(input.color.rgb,input.color.a*fx_alpha(input));
}
]]),
	dither = shader([[
fn fx_effect(input:LayerInput,g:vec3<f32>,i:vec2<u32>)->vec3<f32> {
 var bayer=array<u32,16>(0u,8u,2u,10u,12u,4u,14u,6u,3u,11u,1u,9u,15u,7u,13u,5u);
 let b=i/2u;
 let c=fx_src(input,g,b*2u+1u);
 let t=f32(bayer[(b.y%4u)*4u+b.x%4u]);
 let peak=max(max(c.r,c.g),c.b);
 return select(c*.08,c/max(peak,1./255.),peak>(t+.5)/16.);
}
]]),
	ascii = shader([[
// Rows of five, top to bottom: ` .:-=+*#%@`.
fn fx_glyph(i:u32,row:u32)->u32 {
 var g=array<u32,7>(0u,0u,0u,0u,0u,0u,0u);
 switch i {
  case 1u: {g=array<u32,7>(0u,0u,0u,0u,0u,4u,0u);}
  case 2u: {g=array<u32,7>(0u,4u,0u,0u,4u,0u,0u);}
  case 3u: {g=array<u32,7>(0u,0u,0u,14u,0u,0u,0u);}
  case 4u: {g=array<u32,7>(0u,0u,14u,0u,14u,0u,0u);}
  case 5u: {g=array<u32,7>(0u,4u,4u,31u,4u,4u,0u);}
  case 6u: {g=array<u32,7>(0u,21u,14u,31u,14u,21u,0u);}
  case 7u: {g=array<u32,7>(10u,10u,31u,10u,31u,10u,10u);}
  case 8u: {g=array<u32,7>(17u,2u,4u,4u,8u,16u,17u);}
  case 9u: {g=array<u32,7>(14u,17u,23u,21u,23u,16u,14u);}
  default: {}
 }
 return g[min(row,6u)];
}
fn fx_effect(input:LayerInput,g:vec3<f32>,i:vec2<u32>)->vec3<f32> {
 let at=vec2(i.x/6u*6u+3u,i.y/8u*8u+4u);
 let gc=fx_src(input,g,at);
 let l=fx_luma(gc);
 let t=input.params[0].y;
 let ink=clamp((select(l,1.-l,fx_light(input))-t)/max(1.-t,.01),0.,1.);
 let k=min(u32(sqrt(ink)*9.),9u);
 let cx=i.x%6u; let cy=i.y%8u;
 var lit=false;
 if cx<5u && cy<7u {lit=((fx_glyph(k,cy)>>(4u-cx))&1u)==1u;}
 return fx_src(input,g,i)*.6+select(fx_paper(input),gc,lit)*.4;
}
]]),
	halftone = shader([[
fn fx_effect(input:LayerInput,g:vec3<f32>,i:vec2<u32>)->vec3<f32> {
 let b=i/4u*4u;
 let l=fx_luma(fx_src(input,g,b));
 let tone=select(l,1.-l,fx_light(input));
 let radius=2.*(.3+.7*sqrt(tone));
 let cover=clamp(radius+.5-length(vec2<f32>(i-b)-1.5),0.,1.);
 let dot=fx_src(input,g,b+2u);
 return fx_src(input,g,i)*.6+mix(fx_paper(input),dot,cover)*.4;
}
]]),
	scanlines = shader([[
fn fx_effect(input:LayerInput,g:vec3<f32>,i:vec2<u32>)->vec3<f32> {
 let c=fx_src(input,g,i);
 let gain=select(1.,.52,i.y%3u==0u);
 return select(c*gain,c+(vec3(1.)-c)*(1.-gain),fx_light(input));
}
]]),
}) :: { [string]: any }

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
-- the threshold and 3 faded, to set in place.
function M.params(o: Params): { any }
	local p = o.picture
	local gw, gh = M.grid(p)
	return {
		{ o.strength or 1, o.threshold or 0, if o.light then 1 else 0, if o.faded == false then 0 else 1 },
		{ gw, gh, if p then math.clamp(p.x or 0.5, 0, 1) else 0.5, if p then math.clamp(p.y or 0.5, 0, 1) else 0.5 },
		{ if p then p.zoom or 1 else 1, 0, 0, 0 },
		{ 0, 0, 0, 0 },
	}
end

return M
