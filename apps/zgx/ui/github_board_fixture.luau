-- GitHub's pull request board (plugins-builtin/github/board.luau) in its
-- page's place, as the page area draws one, with the design system set up
-- as the app sets it up: for the board's tests (src/plugins/examples_tests.rs)
-- and its pictures (src/audit_shots.rs, `AUDIT_PLUGINS`). The host plays
-- GitHub, the records and storage.

local ds = require("@ds")
ds.setup()

local light = ui.state("github_fixture.light", false)

return ui.column {
	id = "root",
	w_full = true,
	h_full = true,
	theme = function(): string
		return if light:get() then "zeron-light" else "zeron-dark"
	end,
	bg = "$zeron.window",
	ui.column {
		w_full = true,
		h_full = true,
		provide = { page = { id = "pulls", title = "Pull requests" }, panel = { page = { plugin = "github", id = "pulls" } } },
		ui.column { id = "slot", class = "plugin-page plugin-github", w_full = true, h_full = true, ui.text { "placeholder" } },
	},
}
