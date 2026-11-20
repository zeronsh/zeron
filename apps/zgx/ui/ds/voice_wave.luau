-- The voice bar's waveform, following the microphone: one bar a 60 ms slot,
-- the newest at the trailing edge, older ones scrolling toward the leading
-- edge, where they fade; a quiet slot is a dot on the baseline. The old
-- app's proportions (crates/ui/src/dictation/waveform.rs): 3 point bars, 3
-- point gaps, the quietest bar 0.08 of the height.
--
--   VoiceWave { levels = State<{ number }>, phase = State<string> }
--
-- `levels`: the bars, newest first, 0 to 1 (screens/chat/dictation's, from
-- the host as each slot fills). `phase`: "starting" (the microphone opening:
-- a quiet baseline breathes), "listening" (the bars), "finalizing" (the bars
-- hold while a highlight sweeps across them). Scripts run only as the levels
-- change, once a slot; the breath and the sweep are the engine's.

local M = {}

M.BAR, M.GAP, M.FLOOR, M.H = 3, 3, 0.08, 16
-- Bars drawn (7.7 s of them, 768 points): the host keeps as many. Past
-- them, the baseline's dots go on to the leading edge.
M.COUNT = 128
-- An empty slot's dot, and the bars at rest under the sweep, against the ink.
M.QUIET = 0.35

-- A bar's height for its level (nil: no slot there yet, a dot).
function M.height(level: number?): number
	local a = if level then math.clamp(level, 0, 1) else 0
	return math.max(M.BAR, (M.FLOOR + (1 - M.FLOOR) * a) * M.H)
end

-- The baseline: dots from the trailing edge, on the bars' pitch. Waiting,
-- a soft swell travels along them toward the leading edge. params[0]: the
-- ink; params[1]: the first dot drawn (from the trailing edge) and the swell
-- (1 waiting; 0 a still baseline past the bars).
local BASELINE = ui.shader([[
fn fill(input: FillInput) -> vec4<f32> {
 let ink = input.params[0];
 let from_end = input.size.x - input.p.x;
 let k = floor(from_end / 6.0);
 if (k < input.params[1].x) { return vec4(0.0); }
 let cx = from_end - k * 6.0 - 1.5;
 let wave = input.time / 1.4 - k / 18.0;
 let swell = input.params[1].y * pow(0.5 + 0.5 * sin(wave * 6.2831853), 3.0);
 let h = max(3.0, (0.08 + 0.17 * swell) * input.size.y);
 let y = abs(input.p.y - input.size.y * 0.5);
 let d = length(vec2(cx, max(y - (h - 3.0) * 0.5, 0.0))) - 1.5;
 let a = clamp(0.5 - d * input.scale, 0.0, 1.0);
 return vec4(ink.rgb, ink.a * a * (0.35 + 0.4 * swell));
}
]], { fps = 30 })

-- Transcribing: the held bars rest quiet while a highlight crosses them
-- from the trailing edge, once every 1.6 s.
local SWEEP = ui.layer_shader([[
fn layer(input: LayerInput) -> vec4<f32> {
 let sweep = fract(input.time / 1.6);
 let position = 1.0 - input.p.x / input.size.x;
 let g = clamp(1.0 - abs(position - (sweep * 1.4 - 0.2)) / 0.18, 0.0, 1.0);
 let glow = 1.0 - pow(1.0 - g, 3.0);
 return vec4(input.color.rgb, input.color.a * mix(0.35, 1.0, glow));
}
]], { fps = 30 })

local INK = "$zeron.composer.voice.ink"

-- Bar `i` from the trailing edge (1: the newest).
local function bar(levels: any, i: number): Node
	return ui.div {
		id = `voice-bar-{i}`,
		w = M.BAR,
		shrink = 0,
		rounded = "$radius.full",
		bg = INK,
		h = function(): number
			return M.height(levels:get()[i])
		end,
		opacity = function(): number
			return if levels:get()[i] ~= nil then 1 else M.QUIET
		end,
	}
end

M.VoiceWave = ui.component(function(props: { levels: any, phase: any }): Node
	local levels, phase = props.levels, props.phase
	local function waiting(): boolean
		return phase:get() == "starting"
	end
	local row: { [any]: any } = {
		absolute = true,
		right = 0,
		top = 0,
		bottom = 0,
		items = "center",
		gap = M.GAP,
	}
	for i = M.COUNT, 1, -1 do
		table.insert(row, bar(levels, i))
	end
	-- (Its ink as a colour, not a token: a clocked shader's parameters drop
	-- theme tokens, zgx 4809bcce. The composer's presence is keyed on the
	-- theme, so this is made again as that changes.)
	local ink = ui.theme.get("zeron.composer.voice.ink")
	local function baseline(first: number, breathe: boolean): Node
		return ui.div {
			absolute = true,
			left = 0,
			right = 0,
			top = 0,
			bottom = 0,
			bg_shader = BASELINE,
			shader_clock = breathe,
			shader_params = { ink, { first, if breathe then 1 else 0, 0, 0 }, { 0, 0, 0, 0 }, { 0, 0, 0, 0 } },
		}
	end
	return ui.div {
		id = "voice-wave",
		grow = 1,
		h = M.H,
		min_w = 24,
		overflow = "hidden",
		fade_x = { 28, 0 },
		layer_shader = SWEEP,
		layer_clock = true,
		layer_on = function(): boolean
			return phase:get() == "finalizing"
		end,
		ui.show(waiting, baseline(0, true), ui.div {
			absolute = true,
			left = 0,
			right = 0,
			top = 0,
			bottom = 0,
			baseline(M.COUNT, false),
			ui.row(row),
		}),
	}
end)

return M
