-- Effects over a whole picture, as it is composited (so a scene of layers
-- takes one, not a layer each: transparent edges would dither into fringes
-- and the layers' grids into noise).
--
-- `ui.overlay { layer_shader = effects.dither, layer_params = effects.dither_params(), … }`

local decode = require("@self/decode")

local M = {}

-- Dithered as Zeron dithers its wallpapers (crates/ui/src/
-- new_thread_background_effects.rs, `dither_pixels`): square cells `cell`
-- points across, edge to edge, each taking the picture's colour at its
-- centre; where its brightest channel passes the cell's 4×4 Bayer threshold
-- it is lifted to its full value, else it is that colour at 8%. Laid over
-- the picture at `strength` (as Zeron's halftone is), so the picture still
-- shows through.
-- params[0]: the cell (points), the strength (0 to 1).
M.dither = ui.layer_shader([[
fn zd_bayer(c:vec2<u32>)->f32 {
 let m=array<f32,16>(0.,8.,2.,10.,12.,4.,14.,6.,3.,11.,1.,9.,15.,7.,13.,5.);
 return (m[(c.y%4u)*4u+(c.x%4u)]+.5)/16.;
}
fn layer(input:LayerInput)->vec4<f32> {
 let pitch=max(1.,round(input.params[0].x*input.scale))/input.scale;
 let cell=floor(input.p/pitch);
 let c=zgx_sample((cell+.5)*pitch);
 let peak=max(max(c.r,c.g),c.b);
 let gain=select(.08,1./max(peak,.004),peak>zd_bayer(vec2<u32>(cell)));
 let dot=min(c.rgb*gain,vec3(1.));
 return vec4(mix(input.color.rgb,dot,input.params[0].y),input.color.a);
}
]])

-- The dither's parameters: cells `cell` points across (Zeron's: 2), laid
-- over the picture at `strength` (half, by default).
function M.dither_params(cell: number?, strength: number?): { any }
	return { { cell or 2, strength or 0.5, 0, 0 }, { 0, 0, 0, 0 }, { 0, 0, 0, 0 }, { 0, 0, 0, 0 } }
end

-- Drawn in characters, as one picture: cells of the 5 by 7 font (each font
-- pixel `px` points, a cell 6 by 8 of them), sparse: a cell darker than
-- `threshold` holds none (the picture as it is), the rest a glyph of
-- ` .-+xXS80#@` rising steeply with the brightness at its centre (the
-- highlights dense, the mid tones light), its strokes lit in that colour at
-- its full value over the picture as it is; laid over it at `strength`.
-- params[0]: the font pixel (points), the strength (0 to 1), the threshold
-- (0 to 1).
M.ascii = ui.layer_shader(decode.GLYPHS .. [[
fn layer(input:LayerInput)->vec4<f32> {
 let px=max(1.,round(input.params[0].x*input.scale))/input.scale;
 let size=vec2(6.,8.)*px;
 let cell=floor(input.p/size);
 let c=zgx_sample((cell+.5)*size);
 let lum=dot(c.rgb,vec3(.2126,.7152,.0722));
 let peak=max(max(c.r,c.g),c.b);
 let t=input.params[0].z;
 let level=pow(clamp((lum-t)/max(1.-t,.01),0.,1.),1.4);
 let i=u32(level*10.999);
 // No glyph here: the picture, untouched.
 if i==0u {return input.color;}
 let g=glyph(i);
 let fp=vec2<u32>(floor((input.p-cell*size)/px));
 var lit=0u;
 if fp.x<5u && fp.y<7u {lit=(select(g.y,g.x>>(fp.y*5u),fp.y<6u)>>(4u-fp.x))&1u;}
 // Only the glyph's own pixels: the cell around them stays the picture
 // (darkening it would box every character in).
 let art=select(input.color.rgb,c.rgb/max(peak,.004),lit==1u);
 return vec4(mix(input.color.rgb,art,input.params[0].y),input.color.a);
}
]])

-- The characters' parameters: a font pixel `px` points (1.5, by default),
-- laid over the picture at `strength` (0.75), none in cells darker than
-- `threshold` (0.22).
function M.ascii_params(px: number?, strength: number?, threshold: number?): { any }
	return { { px or 1.5, strength or 0.75, threshold or 0.22, 0 }, { 0, 0, 0, 0 }, { 0, 0, 0, 0 }, { 0, 0, 0, 0 } }
end

-- Printed in dots, as Zeron's halftone (new_thread_background_effects.rs,
-- `halftone_pixels`): square cells `cell` points across, each a round dot
-- of the colour at its centre, wider where the picture is brighter (in
-- the light, where it's darker), on paper: black in the dark, white in
-- the light. Laid over the picture at `strength`, so it still shows
-- through. params[0]: the cell (points), the strength (0 to 1), unused,
-- light (1) or dark (0).
M.halftone = ui.layer_shader([[
fn layer(input:LayerInput)->vec4<f32> {
 let pitch=max(2.,round(input.params[0].x*input.scale))/input.scale;
 let cell=floor(input.p/pitch);
 let centre=(cell+.5)*pitch;
 let c=zgx_sample(centre);
 let light=input.params[0].w>.5;
 var lum=dot(c.rgb,vec3(.2126,.7152,.0722));
 if light {lum=1.-lum;}
 let radius=pitch*.5*(.3+.7*sqrt(clamp(lum,0.,1.)));
 let cover=clamp((radius-distance(input.p,centre))*input.scale+.5,0.,1.)*c.a;
 let paper=select(vec3(0.),vec3(1.),light);
 let print=mix(paper,c.rgb,cover);
 return vec4(mix(input.color.rgb,print,input.params[0].y),input.color.a);
}
]])

-- The halftone's parameters: cells `cell` points across (4, by default),
-- laid at `strength` (0.6), on light paper or dark.
function M.halftone_params(cell: number?, strength: number?, light: boolean?): { any }
	return { { cell or 4, strength or 0.6, 0, if light then 1 else 0 }, { 0, 0, 0, 0 }, { 0, 0, 0, 0 }, { 0, 0, 0, 0 } }
end

-- A display's lines, as Zeron's scanlines (`scanline_pixels`): every
-- `pitch` points a line a third of it tall, darkened (in the light,
-- lifted towards white) by `strength`; the picture between them as it is.
-- params[0]: the pitch (points), the strength (0 to 1), unused, light (1)
-- or dark (0).
M.scanlines = ui.layer_shader([[
fn layer(input:LayerInput)->vec4<f32> {
 let pitch=max(3.,round(input.params[0].x*input.scale));
 let row=floor(input.p.y*input.scale);
 let line=(row%pitch)<max(1.,round(pitch/3.));
 if !line {return input.color;}
 let k=input.params[0].y;
 let c=input.color.rgb;
 let out=select(c*(1.-k),c+(vec3(1.)-c)*k,input.params[0].w>.5);
 return vec4(out,input.color.a);
}
]])

-- The lines' parameters: every `pitch` points (3, by default), at
-- `strength` (0.48: Zeron's), on light or dark.
function M.scanlines_params(pitch: number?, strength: number?, light: boolean?): { any }
	return { { pitch or 3, strength or 0.48, 0, if light then 1 else 0 }, { 0, 0, 0, 0 }, { 0, 0, 0, 0 }, { 0, 0, 0, 0 } }
end

return M
