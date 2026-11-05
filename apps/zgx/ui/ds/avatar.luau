-- Someone's avatar, before they have a picture: a wavy gradient of their
-- own, from their id. Three of the palette's hues (neighbours on the wheel,
-- so they blend rather than clash) wash through each other in bands that
-- bend; the hues, the bands' angles, how tightly they wave and where are all
-- drawn from a hash of the id, so a person is always the same one and two
-- people almost never alike. Still: it costs no frames.
--
-- `Avatar { seed = user.id, size = 20, label = user.name }`. `cut`: a circle
-- taken out of it (another face over it, and the gap round that face), so
-- what is behind shows through the gap, glass or not: `{ x, r }` in points,
-- its centre `x` along the avatar's middle.
--
-- `src`: their own photo (a file's path, or a function of one; nil or ""
-- for none): shown in the circle once it loads, the waves standing in while
-- it does and if it can't. (A photo is a swap of its own: a new one
-- crossfades in.)

local palette = require("@self/palette")

export type Props = {
	seed: string,
	size: number?,
	label: Label?,
	cut: { x: number, r: number }?,
	src: (string | () -> string?)?,
}

-- The hues it draws from, round the wheel (no gray), and the steps it takes
-- them at: a light, a middle and a deep, so the waves have depth.
local WHEEL = {
	"red", "orange", "amber", "yellow", "lime", "green", "emerald", "teal",
	"cyan", "sky", "blue", "indigo", "violet", "purple", "fuchsia", "pink", "rose",
}
local STEPS = { "300", "500", "600" }

-- params[0..2]: its three colours (their alphas, the cut: its centre and
-- radius as fractions of the width, and whether there is one); [3]: the
-- bands' angle and tightness, and their phases (its seed, as numbers).
local WAVES = ui.shader([[
fn fill(input:FillInput)->vec4<f32> {
 let uv=input.p/max(input.size,vec2(1.))-.5;
 let s=input.params[3];
 let a=s.x*6.2831853;
 let r=vec2(cos(a)*uv.x-sin(a)*uv.y,sin(a)*uv.x+cos(a)*uv.y);
 let k=2.2+s.y*2.4;
 let t1=.5+.5*sin(r.x*k+s.z*6.2831853+1.6*sin(r.y*k*1.3+s.w*6.2831853));
 let t2=.5+.5*sin(r.y*k*.9-s.w*6.2831853+1.3*sin(r.x*k*1.1+s.z*6.2831853));
 let c=mix(mix(input.params[0].rgb,input.params[1].rgb,smoothstep(.15,.85,t1)),input.params[2].rgb,.65*smoothstep(.3,.95,t2));
 // The cut, edged over half a point either side.
 let w=input.size.x;
 let d=distance(input.p,vec2(input.params[0].w*w,.5*input.size.y))-input.params[1].w*w;
 let keep=mix(1.,smoothstep(-.5,.5,d),input.params[2].w);
 return vec4(c,keep);
}
]])

-- FNV-1a over the id's bytes, then a little generator off it.
local function hash(s: string): number
	local h = 2166136261
	for i = 1, #s do
		h = bit32.bxor(h, string.byte(s, i))
		-- × 16777619 (2^24 + 403), mod 2^32, exactly (a plain multiply would
		-- pass 2^53 and round).
		h = (bit32.lshift(h, 24) + h * 403) % 4294967296
	end
	return h
end

-- One channel of a "#rrggbb", 0 to 1 (the palette's own colours: a bad
-- one is an error, as ever).
local function channel(hex: string, at: number): number
	local n = tonumber(string.sub(hex, at, at + 1), 16)
	assert(n, "not a #rrggbb colour")
	return n / 255
end

local function rgb(hex: string): { number }
	return { channel(hex, 2), channel(hex, 4), channel(hex, 6), 1 }
end

local function waves(props: Props): Node
	local size = props.size or 20
	local state = hash(props.seed)
	local function next(): number
		state = (state * 1664525 + 1013904223) % 4294967296
		return state / 4294967296
	end
	-- A base hue, and its neighbours either side.
	local base = math.floor(next() * #WHEEL)
	local function hue(offset: number): string
		return WHEEL[(base + offset) % #WHEEL + 1]
	end
	local colors = {
		rgb((palette :: any)[hue(0)][STEPS[1]]),
		rgb((palette :: any)[hue(1)][STEPS[2]]),
		rgb((palette :: any)[hue(-1)][STEPS[3]]),
	}
	-- (The cut rides in the colours' alphas, which the waves don't use.)
	local cut = props.cut
	colors[1][4] = if cut then cut.x / size else 0
	colors[2][4] = if cut then cut.r / size else 0
	colors[3][4] = if cut then 1 else 0
	return ui.div {
		w = size,
		h = size,
		shrink = 0,
		rounded = "$radius.full",
		overflow = "hidden",
		label = props.label,
		bg_shader = WAVES,
		shader_params = { colors[1], colors[2], colors[3], { next(), next(), next(), next() } },
	}
end

return function(props: Props): Node
	local given = props.src
	if not given then
		return waves(props)
	end
	local size = props.size or 20
	local function src(): string
		local v = if type(given) == "function" then given() else given
		return v or ""
	end
	return ui.presence(src, function(s: string): Node
		if s == "" then
			return waves(props)
		end
		return ui.div {
			w = size,
			h = size,
			shrink = 0,
			rounded = "$radius.full",
			overflow = "hidden",
			label = props.label,
			ui.image {
				src = s,
				w_full = true,
				h_full = true,
				fit = "cover",
				aria_hidden = true,
				-- (While a face from the web comes, or one that can't be
				-- drawn: its waves.)
				fallback = waves({ seed = props.seed, size = size }),
			},
		}
	end, { w = size, h = size, shrink = 0, enter = { opacity = { 0, 1 }, duration = 160 }, exit = { opacity = 0, duration = 80 } })
end
