-- A model picker on an agent reaching models through two providers (Pi
-- signed in to Codex and OpenCode, listing GPT-5.5 from each), for
-- tests/model_picker_sources.test.luau: namesakes, a model only one
-- provider has, a long name from both, and an agent with no providers.
-- The model chosen is `test_picker_model`; `test_picker_open` bumped opens
-- it. Here, beside app.luau, as a mounted script loads modules from its own
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
local CODEX = { id = "openai-codex", name = "Codex" }
local OPENCODE = { id = "opencode", name = "OpenCode" }
local LONG = "GPT-5.5 Codex Max Preview Extended Context"

local MODELS: { ModelPicker.Model } = {
	{ value = "pi/openai-codex/gpt-5.5", label = "GPT-5.5", harness = "pi", provider = CODEX, efforts = THREE },
	{ value = "pi/opencode/gpt-5.5", label = "GPT-5.5", harness = "pi", provider = OPENCODE, efforts = THREE },
	{ value = "pi/openai-codex/gpt-5.5-mini", label = "GPT-5.5 mini", harness = "pi", provider = CODEX },
	{ value = "pi/opencode/big-pickle", label = "Big Pickle", harness = "pi", provider = OPENCODE },
	{ value = "pi/openai-codex/long", label = LONG, harness = "pi", provider = CODEX },
	{ value = "pi/opencode/long", label = LONG, harness = "pi", provider = OPENCODE },
	{ value = "claude/opus", label = "Opus", harness = "claude", efforts = THREE },
	-- The same name as Pi's, in another agent: its tab tells them apart.
	{ value = "claude/gpt-5.5", label = "GPT-5.5", harness = "claude" },
}

local HARNESSES: { ModelPicker.Harness } = {
	{ id = "pi", label = "Pi", mark = "pi" },
	{ id = "claude", label = "Claude", mark = "claude" },
}

return ui.column {
	w_full = true,
	h_full = true,
	p = "$space.4",
	ModelPicker {
		model = ui.state("test_picker_model", "pi/openai-codex/gpt-5.5" :: string?),
		effort = ui.state("test_picker_effort", "medium" :: string?),
		favorites = ui.state("test_picker_favorites", {} :: { [string]: boolean }),
		open_with = ui.state("test_picker_open", 0),
		harnesses = HARNESSES,
		models = ui.state("test_picker_models", MODELS),
	},
}
