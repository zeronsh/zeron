-- An agent's presence: the light sweep. Zeron's mark (5 × 3 tiles, the
-- main agent) or the sub-agent's dot (3 × 3), its tiles squircles on the
-- mark's grid, a band of light three diagonals wide crossing it from the
-- top left, a step at a time, then a step at rest. Nothing moves: a tile is
-- lit or at rest, in its state's hue (at rest, faint: lighter than the
-- brand's renders, so the glyph reads as itself beside text).
--
--   Sweep { who = "agent", size = 14, pace = "working" }
--
-- `pace`: "working" (a step every sixth of a second), "connecting" (half
-- as fast), or "still" (every tile at rest: waiting). With motion reduced a
-- sweep stands still mid-way.
--
-- Cheap: one quad on the GPU, the tiles' faces a bitmask a step, the step
-- the shader's time, set only as the step changes (six times a second, by
-- ds/status's one timer, so every sweep on screen steps on the same frame;
-- out of sight, it stops).

local M = {}

-- The space between tiles, in tiles: 1/φ⁴, as the mark.
local GAP = 1 / ((1 + math.sqrt(5)) / 2) ^ 4

-- Each glyph's tiles, and where its band starts (in diagonals): the mark's
-- runs on it at once; the dot's comes in from before it, so its first and
-- last steps light one tile alone.
local GLYPHS = {
	agent = { rows = { "###.#", "##.##", "#.###" }, from = 0, still = 4 },
	subagent = { rows = { "##.", "#.#", ".##" }, from = -1, still = 3 },
}

-- A tile's bit: row by row, left to right.
local function bits(rows: { string }, lit: (r: number, c: number) -> boolean): number
	local n, i = 0, 0
	for r, row in rows do
		for c = 1, #row do
			if row:sub(c, c) == "#" and lit(r - 1, c - 1) then
				n += 2 ^ i
			end
			i += 1
		end
	end
	return n
end

-- The lit tiles at each of the seven lit steps (the eighth rests).
local function steps(g: { rows: { string }, from: number }): { number }
	local out = {}
	for s = 0, 6 do
		local band = s + g.from
		out[s + 1] = bits(g.rows, function(r, c)
			return math.abs(r + c - band) <= 1
		end)
	end
	return out
end
local function shape(g: { rows: { string } }): number
	return bits(g.rows, function()
		return true
	end)
end
for _, g in GLYPHS do
	g.steps = steps(g)
	g.shape = shape(g)
end
M.GLYPHS = GLYPHS

-- params[0]: the lit face's ink (a token: zgx resolves one only there);
-- params[1].x: the rest's share of its alpha.
-- params[2], params[3].xyz: the lit tiles of steps 0 to 6 (step 7 rests);
-- params[3].w: the glyph (0 the mark, 1 the dot). The step is the time's
-- nearest whole number, so a frame sampled on the step's own beat (a
-- baked loop's) lands in it.
local SOURCE = string.format(
	[[
fn fill(input:FillInput)->vec4<f32> {
 let dot=input.params[3].w>.5;
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
 let s=u32(floor(input.time+.5))%%8u;
 var lit=0u;
 if s<4u { lit=u32(input.params[2][s]); } else if s<7u { lit=u32(input.params[3][s-4u]); }
 let ink=input.params[0];
 let a=select(input.params[1].x,1.,((lit>>i)&1u)==1u);
 return vec4(ink.rgb,ink.a*a*cover);
}
]],
	GLYPHS.agent.shape,
	GLYPHS.subagent.shape,
	GAP
)
-- (Compiled as the first sweep is drawn.)
local shader: any = nil

-- A step's length (ms): a sixth of a second, ten frames of a 60 Hz display,
-- so a step is drawn once, on the display's beat. Connecting goes at half
-- the pace, its steps on every other of working's.
M.STEP = 1000 / 6
local PACES = { working = 1, connecting = 2 }

-- A tile at rest, as a share of a lit one's ink: lighter than the brand's
-- renders (0.15 there, on its ink), so the unlit part reads in both themes.
M.REST = 0.4

local function params(ink: string, lit: { number }, dot: boolean): { any }
	return {
		ink,
		{ M.REST, 0, 0, 0 },
		{ lit[1], lit[2], lit[3], lit[4] },
		{ lit[5], lit[6], lit[7], if dot then 1 else 0 },
	}
end

export type Props = {
	who: ("agent" | "subagent")?,
	size: number?,
	pace: ("working" | "connecting" | "still")?,
	ink: string?,
	-- One step held (0 to 7, 7 at rest): a still frame, for a sheet.
	at: number?,
	-- Motion reduced (ds/motion): it stands still mid-way.
	reduced: boolean?,
}

-- The lit tiles of each of its seven lit steps, and how many of the
-- beat's steps (`STEP`) each lasts, or none when it stands still: every
-- tile at rest (waiting) or, with motion reduced, the band mid-way across.
-- (Its step k is drawn at shader time k.)
function M.plan(props: Props): ({ number }, number?)
	local g = GLYPHS[props.who or "agent"]
	local pace = props.pace or "working"
	if pace == "still" or props.at or props.reduced then
		local at = props.at or (if pace == "still" then 7 else g.still)
		local face = g.steps[at + 1] or 0
		return { face, face, face, face, face, face, face }, nil
	end
	return g.steps, PACES[pace]
end

-- The sweep's node, and its steps' length in beats (as `plan`).
function M.node(props: Props): (ContainerProps, number?)
	local size = props.size or 14
	local lit, every = M.plan(props)
	shader = shader or ui.shader(SOURCE)
	return {
		w = size,
		h = size,
		shrink = 0,
		bg_shader = shader,
		shader_params = params(props.ink or "$zeron.status.working", lit, props.who == "subagent"),
	},
		every
end

return M
