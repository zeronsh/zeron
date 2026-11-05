-- GitHub's pull request viewer (plugins-builtin/github/viewer.luau) in a
-- plugin tab's place, as the panel draws one, with the design system set up
-- as the app sets it up (its tokens declared, its themes loaded): for the
-- viewer's tests (src/plugins/examples_tests.rs) and its pictures
-- (src/audit_shots.rs, `AUDIT_PLUGINS`). The host plays GitHub.
--
-- `github_fixture.width` narrows the tab to a side pane's width (0: the
-- window's); `github_fixture.light` draws it light; `github_fixture.template`
-- ("pink") lays a template's few tokens over Zeron's (a hot pink, as
-- people's own do: only some tokens set).

local ds = require("@ds")
ds.setup()

local width = ui.state("github_fixture.width", 0)
local template = ui.state("github_fixture.template", "")
local light = ui.state("github_fixture.light", false)
-- The pull request it shows (a real one, with real gh: any address).
local address = ui.state("github_fixture.url", "https://github.com/zeronsh/zgx/pull/1264")

-- A template that sets only some tokens, over each of Zeron's themes: its
-- accent, its window and its cards (a hot pink), the rest Zeron's own.
ui.theme.register {
	name = "pink-light",
	type = "light",
	extends = "zeron-light",
	tokens = {
		accent = "#ff2d95",
		background = "#ffe3f1",
		zeron = { window = "#ffe3f1", settings = { card = "#fff3f9", edge = "#ffb3d6", rule = "#ffd0e6" } },
	},
}
ui.theme.register {
	name = "pink-dark",
	type = "dark",
	extends = "zeron-dark",
	tokens = {
		accent = "#ff4fa8",
		background = "#24101c",
		zeron = { window = "#24101c", settings = { card = "#341628", edge = "#ff4fa83d", rule = "#ff4fa826" } },
	},
}

-- (Drawn again as the template, the scheme or the width changes: the tab's
-- slot is the plugin's again, as a panel's is when it opens.)
return ui.presence(function(): string
	return `{template:get()}|{light:get()}|{width:get()}|{address:get()}`
end, function(): Node
	local w = width:peek()
	local scheme = if light:peek() then "light" else "dark"
	return ui.column {
		id = "root",
		w_full = true,
		h_full = true,
		items = "center",
		theme = if template:peek() == "pink" then `pink-{scheme}` else `zeron-{scheme}`,
		bg = "$zeron.window",
		ui.column {
			w = if w > 0 then ui.raw(w, "a side pane's width") else nil,
			w_full = w == 0,
			h_full = true,
			provide = { tab = { kind = "github-pr", title = "#1264", data = { url = address:peek(), session = "s1" } } },
			ui.column { id = "slot", class = "plugin-tab plugin-github", w_full = true, h_full = true, ui.text { "placeholder" } },
		},
	}
end, { w_full = true, h_full = true })
