-- An agent's presence: the light sweep. Zeron's mark (5 × 3 tiles, the
-- main agent) or the sub-agent's dot (3 × 3), its tiles squircles on the
-- mark's grid, a band of light three diagonals wide crossing it from the
-- top left, then a moment at rest. Nothing moves: a tile is lit, at rest,
-- or half way as the band's soft edge passes, in its state's hue (at rest,
-- faint: lighter than the brand's renders, so the glyph reads as itself
-- beside text).
--
--   Sweep { who = "agent", size = 14, pace = "working" }
--
-- `pace`: "working" (the mark crosses in a second, the dot a little
-- quicker), "connecting" (half as fast), or "still" (every tile at rest:
-- waiting). With motion reduced a sweep stands still mid-way.
--
-- Cheap: one quad on the GPU, the band's place the shader's time, set only
-- as it moves (twelve times a second, by ds/status's one timer, so every
-- sweep on screen moves on the same frame; out of sight, it stops).

local M = {}

-- The space between tiles, in tiles: 1/φ⁴, as the mark.
local GAP = 1 / ((1 + math.sqrt(5)) / 2) ^ 4

-- Each glyph's tiles; its cycle in beats (`BEAT`), the last `rest` of them
-- dark; where its band stands still (motion reduced: mid-way, through the
-- gap).
local GLYPHS = {
	agent = { rows = { "###.#", "##.##", "#.###" }, beats = 12, rest = 2, still = 4 },
	subagent = { rows = { "##.", "#.#", ".##" }, beats = 10, rest = 2, still = 2 },
}

-- A tile's bit: row by row, left to right.
local function shape(rows: { string }): number
	local n, i = 0, 0
	for _, row in rows do
		for c = 1, #row do
			if row:sub(c, c) == "#" then
				n += 2 ^ i
			end
			i += 1
		end
	end
	return n
end
for _, g in GLYPHS do
	g.shape = shape(g.rows)
	-- (Its diagonals, 0 at the top left.)
	g.last = #g.rows[1] - 1 + #g.rows - 1
end
M.GLYPHS = GLYPHS

-- Where the band stands (in diagonals) at a cycle's beat `i`: from just
-- before the first diagonal to just past the last, evenly, then off the
-- glyph for its rest.
local OFF = -9
function M.band(who: "agent" | "subagent", i: number): number
	local g = GLYPHS[who]
	local moving = g.beats - g.rest
	i %= g.beats
	if i >= moving then
		return OFF
	end
	return -1 + (g.last + 2) * i / (moving - 1)
end

-- params[0]: the lit face's ink (a token: zgx resolves one only there);
-- params[1]: the rest's share of its alpha, the glyph (0 the mark, 1 the
-- dot), where a still band stands and whether it does (else it stands at
-- the shader's time). A tile within a diagonal of the band's middle is lit,
-- one a diagonal further half lit, past that at rest.
local SOURCE = string.format(
	[[
fn fill(input:FillInput)->vec4<f32> {
 let dot=input.params[1].y>.5;
 let cols=select(5.,3.,dot);
 let shape=select(%du,%du,dot);
 let g=%.9f;
 let k=min(input.size.x/(cols+(cols-1.)*g),input.size.y/(3.+2.*g));
 let o=(input.size-vec2(cols+(cols-1.)*g,3.+2.*g)*k)*.5;
 let q=(input.p-o)/(k*(1.+g));
 let cell=floor(q);
 if cell.x<0.||cell.y<0.||cell.x>=cols||cell.y>=3. { return vec4(0.); }
 let i=u32(cell.y*cols+cell.x);
 if ((shape>>i)&1u)==0u { return vec4(0.); }
 // A squircle, |x|⁴ + |y|⁴ = 1, the tile's half a side.
 let f=abs(fract(q)*(1.+g)-.5)*2.;
 let f2=f*f;
 let d=sqrt(sqrt(f2.x*f2.x+f2.y*f2.y))-1.;
 let cover=clamp(.5-d*.5*k*input.scale,0.,1.);
 let band=select(input.time,input.params[1].z,input.params[1].w>.5);
 let lit=clamp(2.-abs(cell.x+cell.y-band),0.,1.);
 let ink=input.params[0];
 return vec4(ink.rgb,ink.a*mix(input.params[1].x,1.,lit)*cover);
}
]],
	GLYPHS.agent.shape,
	GLYPHS.subagent.shape,
	GAP
)
-- (Compiled as the first sweep is drawn.)
local shader: any = nil

-- The beat (ms): a twelfth of a second, five frames of a 60 Hz display,
-- so each place of the band is drawn once, on the display's beat; the
-- band's soft edge carries the light between places. Connecting goes at
-- half the pace, on every other beat.
M.BEAT = 1000 / 12
local PACES = { working = 1, connecting = 2 }

-- A tile at rest, as a share of a lit one's ink: lighter than the brand's
-- renders (0.15 there, on its ink), so the unlit part reads in both themes.
M.REST = 0.4

export type Props = {
	who: ("agent" | "subagent")?,
	size: number?,
	pace: ("working" | "connecting" | "still")?,
	ink: string?,
	-- The band held at a place (in diagonals): a still frame, for a sheet.
	at: number?,
	-- Motion reduced (ds/motion): it stands still mid-way.
	reduced: boolean?,
}

-- Where its band stands still (none: it moves), and how many beats each
-- of its places lasts (none: it stands still): every tile at rest
-- (waiting) or, with motion reduced, mid-way across.
function M.plan(props: Props): (number?, number?)
	local g = GLYPHS[props.who or "agent"]
	local pace = props.pace or "working"
	if props.at then
		return props.at, nil
	elseif pace == "still" then
		return OFF, nil
	elseif props.reduced then
		return g.still, nil
	end
	return nil, PACES[pace]
end

-- The sweep's node, and its places' length in beats (as `plan`).
function M.node(props: Props): (ContainerProps, number?)
	local size = props.size or 14
	local still, every = M.plan(props)
	shader = shader or ui.shader(SOURCE)
	return {
		w = size,
		h = size,
		shrink = 0,
		bg_shader = shader,
		shader_params = {
			props.ink or "$zeron.status.working",
			{ M.REST, if props.who == "subagent" then 1 else 0, still or 0, if still then 1 else 0 },
		},
	},
		every
end

return M
