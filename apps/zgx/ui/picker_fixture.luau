-- A model picker on models made up here, for
-- tests/model_picker_in_place.test.luau: two with reasoning (one with Fast
-- mode), one without, and one configured in place (Devin Fusion's shape:
-- its lead and sidekick are options, so it has a card). The model chosen
-- is `test_picker_model`, its options' picks `test_picker_options`. Here,
-- beside app.luau, as a mounted script loads modules from its own
-- directory down.

ui.theme.strict("warn")
local ds = require("@ds")
ds.setup()
local ModelPicker = require("@ds/model_picker")
local EffortSlider = require("@ds/effort_slider")

local THREE: { EffortSlider.Level } = {
	{ value = "low", label = "Low" },
	{ value = "medium", label = "Medium" },
	{ value = "high", label = "High" },
}
local FIVE: { EffortSlider.Level } = {
	{ value = "minimal", label = "Minimal" },
	{ value = "low", label = "Low" },
	{ value = "medium", label = "Medium" },
	{ value = "high", label = "High" },
	{ value = "xhigh", label = "Extra high" },
}

local MODELS: { ModelPicker.Model } = {
	{ value = "sonnet", label = "Sonnet", harness = "claude", efforts = THREE, effort = "medium" },
	{ value = "opus", label = "Opus", harness = "claude", efforts = THREE, effort = "high", fast = true },
	{ value = "gpt", label = "GPT", harness = "codex", efforts = FIVE, effort = "medium" },
	{ value = "mini", label = "Mini", harness = "codex" },
	{ value = "adaptive", label = "Adaptive", harness = "devin" },
	{
		value = "fusion",
		label = "Fusion",
		harness = "devin",
		efforts = THREE,
		effort = "high",
		fast = true,
		options = {
			{ id = "sidekick", label = "Sidekick", default = "swe-medium", choices = { { id = "swe-medium", label = "SWE-2 Medium" }, { id = "swe-high", label = "SWE-2 High" } } },
			{ id = "lead", label = "Lead", default = "fable", choices = { { id = "fable", label = "Fable 5.1" }, { id = "sol", label = "GPT-6 Sol" } } },
		},
	},
}

-- What the picker's panel, and what opens out with its list, have looked
-- like since a test last set this, sampled each frame: the least opacity
-- the panel had, and how many frames what opens out was part see-through
-- (shown or not, never between) (tests/model_picker_motion.test.luau: never
-- see-through as it opens, opens out, folds or follows its trigger).
local alpha = ui.state("test_picker_alpha", { panel = 1, fold = 0 })
ui.every(16, function()
	local now = alpha:peek()
	local panel = tonumber(anime.utils.get(".dropdown-panel", "opacity")) or 1
	local fold = tonumber(anime.utils.get(".dropdown-fold", "opacity")) or 1
	local between = fold > 0 and fold < 1
	if panel < now.panel or between then
		alpha:set({ panel = math.min(panel, now.panel), fold = now.fold + (if between then 1 else 0) })
	end
end)
-- Where the reasoning thumb is drawn (its offset along the groove), each
-- frame.
local thumb = ui.state("test_picker_thumb", -1)
ui.every(16, function()
	local x = tonumber(anime.utils.get(".effort-thumb", "x"))
	if x and x ~= thumb:peek() then
		thumb:set(x)
	end
end)
-- Room over the picker (its trigger moving, as a composer growing over a
-- chip moves it).
local above = ui.state("test_picker_above", 0)

return ui.column {
	w_full = true,
	h_full = true,
	p = "$space.4",
	ui.div {
		w_full = true,
		shrink = 0,
		h = function(): number
			return above:get()
		end,
	},
	ModelPicker {
		model = ui.state("test_picker_model", "sonnet" :: string?),
		effort = ui.state("test_picker_effort", "medium" :: string?),
		fast = ui.state("test_picker_fast", false),
		options = ui.state("test_picker_options", {} :: { [string]: string }),
		favorites = ui.state("test_picker_favorites", {} :: { [string]: boolean }),
		harnesses = {
			{ id = "claude", label = "Claude", mark = "claude" },
			{ id = "codex", label = "Codex", mark = "openai" },
			{ id = "devin", label = "Devin", mark = "devin" },
		},
		models = MODELS,
	},
}
