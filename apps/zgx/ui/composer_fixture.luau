-- The composer, for tests/composer_in_place.test.luau: its notice
-- (`test_composer_notice`) and its project (`test_composer_project`), set
-- as an app would as they change. Here, beside app.luau, as a mounted
-- script loads modules from its own directory down.

ui.theme.strict("warn")
local ds = require("@ds")
local Composer = require("@ds/composer")
ds.setup()

local HARNESSES: { ds.Harness } = { { id = "claude", label = "Claude", mark = "claude" } }
local MODELS: { ds.Model } = { { value = "sonnet", label = "Sonnet", harness = "claude" } }
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
		model = ui.state("test_composer_model", "sonnet" :: string?),
		effort = ui.state("test_composer_effort", "medium" :: string?),
		harnesses = HARNESSES,
		models = MODELS,
		notice = ui.state("test_composer_notice", nil :: Composer.Notice?),
		project = ui.state("test_composer_project", "atlas" :: string?),
		projects = PROJECTS,
	},
}
