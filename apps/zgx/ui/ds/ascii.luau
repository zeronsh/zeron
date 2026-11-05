-- A quiet field of characters (the effort slider's: decode.GLYPHS, ` .-+x
-- XS80#@`, its density wandering with a value noise that drifts left), for
-- an empty space to rest on: sparse, one ink, clearing to nothing around a
-- middle where words sit.
--
-- `ui.div { bg_shader = ascii.QUIET, shader_params = ascii.quiet("$text.tertiary", 0.18, { 120, 26 }),
-- on_mount = ascii.drift }`

local motion = require("@self/motion")
local decode = require("@self/decode")
local prefers = require("@self/prefers")

local M = {}

-- Frames a second it drifts at (the slider's).
local FPS = 12

-- params[0]: ink; [1]: font pixel (points), density, wander, drift (cells a
-- second); [2]: the clearing's half width and half height (points, about
-- the middle), a seed.
M.QUIET = ui.shader(decode.GLYPHS .. [[
fn aq_hash(p:vec2<f32>)->f32 { return fract(sin(dot(p,vec2(127.1,311.7)))*43758.5453); }
fn aq_noise(p:vec2<f32>)->f32 {
 let i=floor(p); let f=fract(p); let u=f*f*(3.-2.*f);
 let a=aq_hash(i); let b=aq_hash(i+vec2(1.,0.));
 let c=aq_hash(i+vec2(0.,1.)); let d=aq_hash(i+vec2(1.,1.));
 return mix(mix(a,b,u.x),mix(c,d,u.x),u.y);
}
fn fill(input:FillInput)->vec4<f32> {
 let ink=input.params[0];
 let r=input.params[1];
 let q=input.params[2];
 let px=max(1.,round(r.x*input.scale))/input.scale;
 let cell=vec2(6.,8.)*px;
 let cols=floor(input.size.x/cell.x);
 let rows=floor(input.size.y/cell.y);
 let o=(input.size-vec2(cols,rows)*cell)*.5;
 let p=input.p-o;
 if p.x<0. || p.y<0. || p.x>=cols*cell.x || p.y>=rows*cell.y {return vec4(0.);}
 let c=floor(p/cell);
 // The clearing: nothing inside its ellipse, easing in past it.
 let mid=(c+.5)*cell+o-input.size*.5;
 let e=length(mid/max(q.xy,vec2(1.)));
 let open=smoothstep(1.,1.8,e);
 var d=r.y+(aq_noise(vec2(c.x*.31-input.time*r.w,c.y*.57+q.z))-.5)*r.z;
 d=clamp(d*open,0.,1.);
 let g=glyph(u32(d*10.999));
 let fp=vec2<u32>(floor((p-c*cell)/px));
 var lit=0.;
 if fp.x<5u && fp.y<7u {lit=f32((select(g.y,g.x>>(fp.y*5u),fp.y<6u)>>(4u-fp.x))&1u);}
 return vec4(ink.rgb,ink.a*lit*(.5+.5*d));
}
]], { fps = FPS })

-- Its parameters: the ink (a token follows the theme), how dense (0 to 1),
-- and the clearing's half extents in points.
function M.quiet(ink: string, density: number, clearing: { number }): { any }
	return { ink, { 1, density, 0.5, 0.35 }, { clearing[1], clearing[2], 7, 0 }, { 0, 0, 0, 0 } }
end

-- Mounted, it drifts (unless motion is reduced).
function M.drift(el: Element)
	if not motion.reduced_now() then
		anime.animate(el, {
			shaderTime = { 0, 600 },
			duration = 600000,
			loop = true,
			ease = "linear",
			frameRate = FPS,
		})
	end
end

return M
