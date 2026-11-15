-- Composer fixture for capability changes while Auto remains selected.
ui.theme.strict("warn")
local ds = require("@ds")
local Composer = require("@ds/composer")
local Cargo = require("@ds/composer_cargo")
local Ask = require("@ds/composer_ask")
ds.setup()

local HARNESSES: { ds.Harness } = { { id = "claude", label = "Claude", mark = "claude" } }
local MODELS: { ds.Model } = { { value = "sonnet", label = "Sonnet", harness = "claude" } }
local access_auto = ui.shared("test_composer_auto", true)
local PROJECTS: { Composer.Project } = {
	{ value = "atlas", label = "Atlas", hue = "violet" },
	{ value = "borealis", label = "Borealis", hue = "blue" },
	{ value = "none", label = "No project" },
}

return ui.column {
	w_full = true,
	h_full = true,
	p = "$space.4",
	justify = "end",
	ds.Composer {
		value = ui.state("test_composer_draft", ""),
		access = ui.shared("test_composer_access", "auto" :: string?),
		access_auto = function(): boolean
			return access_auto:get()
		end,
		model = ui.state("test_composer_model", "sonnet" :: string?),
		effort = ui.state("test_composer_effort", "medium" :: string?),
		harnesses = HARNESSES,
		models = MODELS,
		notice = ui.state("test_composer_notice", nil :: Composer.Notice?),
		dictating = ui.state("test_composer_dictating", false),
		cargo = ui.state("test_composer_cargo", {} :: { Cargo.Item }),
		question = ui.state("test_composer_question", nil :: Ask.Question?),
		project = ui.state("test_composer_project", "atlas" :: string?),
		projects = PROJECTS,
	},
}
