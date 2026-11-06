-- A chart's fills in the effort slider's characters: whatever is drawn under
-- it (bars, areas) is read a cell at a time at the cell's middle and shown in
-- the slider's font and colour, sparse at the low end of the value axis and
-- dense at the high end, its density wandering with the slider's noise as it
-- drifts. (A cell's glyph reaches 3 points across and 4 down from its middle:
-- what's drawn under it sits that far inside the edge it should stay within.)
-- They rise in from the axis once, on mount.
--
-- `ui.div { layer_shader = field.shader, layer_params = field.params(), on_mount = field.mounted, ui.canvas { … } }`

local decode = require("@self/decode")
local motion = require("@self/motion")

local M = {}

-- Frames a second it drifts at (the slider's).
local FPS = 12

-- How long the marks take to rise in.
local RISE = 520

-- Past the top, and the rise's band: all of it shown.
local RISEN = 1.3

-- params[0]: the drift's clock (seconds), the rise (0 to `RISEN`), the
-- density at the low end, across (1: a horizontal chart's axis runs left to
-- right).
M.shader = ui.layer_shader(decode.GLYPHS .. [[
fn cf_hash(p:vec2<f32>)->f32 { return fract(sin(dot(p,vec2(127.1,311.7)))*43758.5453); }
fn cf_noise(p:vec2<f32>)->f32 {
 let i=floor(p); let f=fract(p); let u=f*f*(3.-2.*f);
 let a=cf_hash(i); let b=cf_hash(i+vec2(1.,0.));
 let c=cf_hash(i+vec2(0.,1.)); let d=cf_hash(i+vec2(1.,1.));
 return mix(mix(a,b,u.x),mix(c,d,u.x),u.y);
}
fn layer(input:LayerInput)->vec4<f32> {
 let r=input.params[0];
 let px=max(1.,round(input.scale))/input.scale;
 let size=vec2(6.,8.)*px;
 let cell=floor(input.p/size);
 let mid=(cell+.5)*size;
 let at=zgx_sample(mid);
 if at.a<.015 {return vec4(0.);}
 let along=select(1.-mid.y/input.size.y,mid.x/input.size.x,r.w>.5);
 let u=clamp(along,0.,1.);
 let c=vec2<u32>(max(cell,vec2(0.)));
 // Ahead of the rise, nothing; across its band, characters scrambling.
 let ahead=r.y-u-cell_hash(c)*.12;
 if ahead<0. {return vec4(0.);}
 var d=(r.z+(.96-r.z)*pow(u,1.4))*pow(at.a,.6);
 d+=(cf_noise(vec2(cell.x*.31-r.x*.35,cell.y*.57))-.5)*.3;
 d=clamp(d,0.,1.);
 var index=u32(d*10.999);
 if ahead<.12 {index=1u+u32(cell_hash(c+vec2(u32(r.y*40.),7u))*9.999);}
 let g=glyph(index);
 let fp=vec2<u32>(floor((input.p-cell*size)/px));
 var lit=0.;
 if fp.x<5u && fp.y<7u {lit=f32((select(g.y,g.x>>(fp.y*5u),fp.y<6u)>>(4u-fp.x))&1u);}
 return vec4(at.rgb,lit*(.45+.55*d));
}
]])

function M.params(): { any }
	local start = if motion.reduced_now() then RISEN else 0
	return { { 0, start, 0.16, 0 }, { 0, 0, 0, 0 }, { 0, 0, 0, 0 }, { 0, 0, 0, 0 } }
end

-- Mounted, it rises in and drifts (unless motion is reduced).
function M.mounted(el: Element)
	if motion.reduced_now() then
		return
	end
	anime.animate(el, { layerParam1 = RISEN, duration = RISE, ease = "out(3)" })
	anime.animate(el, {
		layerParam0 = { 0, 600 },
		duration = 600000,
		loop = true,
		ease = "linear",
		frameRate = FPS,
	})
end

return M
