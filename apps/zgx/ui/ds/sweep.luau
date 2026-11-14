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
-- sweep on screen moves on the same frame; out of sight, it stops). Which
-- sweeps move at all is ds/status's: one where you look, the rest still.

local M = {}

-- The space between tiles, in tiles: 1/φ⁴, as the mark.
local GAP = 1 / ((1 + math.sqrt(5)) / 2) ^ 4

-- Each glyph's tiles (its id in the shader); its cycle in beats (ds/motion
-- `presence.beat`), the last `rest` of them dark; where its band stands
-- still (a quiet mark, motion reduced: mid-way, through the gap). The mark
-- at 16 and 14 points, a smaller cut of it to 12 (its gap kept), the
-- sub-agent's dot, and, under 10 points, a micro form of three tiles.
local GLYPHS = {
	agent = { id = 0, rows = { "###.#", "##.##", "#.###" }, beats = 12, rest = 2, still = 4 },
	subagent = { id = 1, rows = { "##.", "#.#", ".##" }, beats = 10, rest = 2, still = 2 },
	small = { id = 2, rows = { "##.#", "#.##" }, beats = 10, rest = 2, still = 2 },
	micro = { id = 3, rows = { "##", "#." }, beats = 8, rest = 2, still = 1 },
}
export type Glyph = "agent" | "subagent" | "small" | "micro"

-- The glyph drawn for `who` at `size` points.
function M.glyph(who: ("agent" | "subagent")?, size: number): Glyph
	if size < 10 then
		return "micro"
	elseif size <= 12 and who ~= "subagent" then
		return "small"
	end
	return who or "agent"
end

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
function M.band(who: Glyph, i: number): number
	local g = GLYPHS[who]
	local moving = g.beats - g.rest
	i %= g.beats
	if i >= moving then
		return OFF
	end
	return -1 + (g.last + 2) * i / (moving - 1)
end

-- params[0]: the lit face's ink (a token: zgx resolves one only there);
-- params[1]: the rest's share of its alpha, the glyph (its id), where a still band stands and whether it does (else it stands at
-- the shader's time). A tile within a diagonal of the band's middle is lit,
-- one a diagonal further half lit, past that at rest.
local SOURCE = string.format(
	[[
fn fill(input:FillInput)->vec4<f32> {
 let id=u32(input.params[1].y+.5);
 var cols=5.; var rows=3.; var shape=%du;
 if id==1u { cols=3.; shape=%du; } else if id==2u { cols=4.; rows=2.; shape=%du; } else if id==3u { cols=2.; rows=2.; shape=%du; }
 let g=%.9f;
 let k=min(input.size.x/(cols+(cols-1.)*g),input.size.y/(rows+(rows-1.)*g));
 let o=(input.size-vec2(cols+(cols-1.)*g,rows+(rows-1.)*g)*k)*.5;
 let q=(input.p-o)/(k*(1.+g));
 let cell=floor(q);
 if cell.x<0.||cell.y<0.||cell.x>=cols||cell.y>=rows { return vec4(0.); }
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
	GLYPHS.small.shape,
	GLYPHS.micro.shape,
	GAP
)
-- (Compiled as the first sweep is drawn.)
local shader: any = nil

local PACES = { working = 1, connecting = 2 }

export type Props = {
	who: ("agent" | "subagent")?,
	size: number?,
	pace: ("working" | "connecting" | "still")?,
	ink: string?,
	-- A tile at rest, as a share of a lit one's ink (ds/motion).
	rest: number?,
	-- The band held at a place (in diagonals): a still frame, for a sheet.
	at: number?,
	-- Motion reduced (ds/motion): it stands still mid-way.
	reduced: boolean?,
}

-- Where its band stands still (none: it moves), and how many beats each
-- of its places lasts (none: it stands still): every tile at rest
-- (waiting) or, with motion reduced, mid-way across.
function M.plan(props: Props): (number?, number?)
	local g = GLYPHS[M.glyph(props.who, props.size or 14)]
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

-- The sweep's node, its places' length in beats (as `plan`), and the glyph
-- it draws.
function M.node(props: Props): (ContainerProps, number?, Glyph)
	local size = props.size or 14
	local glyph = M.glyph(props.who, size)
	local still, every = M.plan(props)
	shader = shader or ui.shader(SOURCE)
	return {
		w = size,
		h = size,
		shrink = 0,
		bg_shader = shader,
		shader_params = {
			props.ink or "$zeron.status.working",
			{ props.rest or 0.4, GLYPHS[glyph].id, still or 0, if still then 1 else 0 },
		},
	},
		every,
		glyph
end

return M
