-- Zeron's Git plugin (plugins-builtin/git) in a plugin tab's place, as the
-- panel draws one, with the design system set up as the app sets it up:
-- for its tests (src/plugins/examples_tests.rs) and its pictures. The
-- host plays the device's Git service (`plugin.git.service_call`) and
-- the session's record (`plugin.git.records`): session "s1".
--
-- `git_fixture.mode`: "tab" (the Git tab) or "empty" (the panel's empty
-- page, where the plugin adds its rows); `git_fixture.view` the tab's
-- view ("changes", "history"); `git_fixture.width` narrows it to a side
-- pane's width (0: the window's); `git_fixture.light` draws it light;
-- `git_fixture.scope` opens it to a scope (`data.scope`).

local ds = require("@ds")
ds.setup()

local mode = ui.state("git_fixture.mode", "tab")
local view = ui.state("git_fixture.view", "changes")
-- With view "commit": the commit its tab was opened with.
local commit = ui.state("git_fixture.commit", nil :: any)
-- For pictures: the capture data's merge commit, in its own tab.
ui.command {
	id = "fixture.open_merge",
	title = "Open the merge commit's tab",
	run = function()
		commit:set({
			sha = "1111111111111111111111111111111111111111",
			subject = "Merge main into feature/graph",
			parents = { "2222222222222222222222222222222222222222", "5555555555555555555555555555555555555555" },
			author = "Ada Lovelace",
			email = "x@example.com",
			at = 1793637057,
			refs = { "HEAD -> feature/graph", "origin/feature/graph" },
		})
		view:set("commit")
	end,
}
local width = ui.state("git_fixture.width", 0)
local light = ui.state("git_fixture.light", false)
-- The scope the tab is opened to ("": none said).
local scope = ui.state("git_fixture.scope", "")
-- The home's letters, as screens/panel provides them: `{ key, n }`, `n`
-- moving on each press.
local panel_key = ui.state("git_fixture.panel_key", { key = "", n = 0 })
-- An item another plugin added to Git's commit menu, chosen as the menu
-- would (`fixture.menu_run`, with `git_fixture.menu_ask` set first).
local menu_ask = ui.state("git_fixture.menu_ask", nil :: any)
ui.command {
	id = "fixture.menu_run",
	title = "Run a commit menu item",
	run = function()
		ui.invoke("plugin.git.menu_run", menu_ask:peek())
	end,
}

-- The comments Git left still on a draft, as screens/plugins tells it
-- (`plugin.git.composed`: key → { session, hovered }): set by the test,
-- standing in for the composer's chips.
local composed = ui.state("git_fixture.composed", {} :: any)
ui.effect(function()
	ui.shared("plugin.git.composed", {} :: any):set(composed:get())
end)
-- For pictures: a comment on lanes.rs's new lines 19–21 carried, quiet
-- (`fixture.comment_marks`) or its chip pointed at (`fixture.comment_lit`).
local function carried(hovered: boolean)
	composed:set({ [ui.json.encode({ p = "crates/graph/src/lanes.rs", k = { "n19", "n20", "n21" } })] = { hovered = hovered } })
end
ui.command {
	id = "fixture.comment_marks",
	title = "A comment carried on lanes.rs",
	run = function()
		carried(false)
	end,
}
ui.command {
	id = "fixture.comment_lit",
	title = "A comment carried on lanes.rs, its chip pointed at",
	run = function()
		carried(true)
	end,
}

-- (Drawn again as any of them changes: the tab's slot is the plugin's
-- again, as a panel's is when it opens.)
return ui.presence(function(): string
	-- (Not its width: that moves in place, as a panel's edge dragged does.)
	return `{mode:get()}|{view:get()}|{light:get()}|{scope:get()}|{(commit:get() or {}).sha or ""}`
end, function(): Node
	local scheme = if light:peek() then "light" else "dark"
	local inner: Node
	if mode:peek() == "empty" then
		-- The panel's empty page, as screens/panel draws it: its own rows in
		-- `hook:panel.empty`, plugins' after them.
		inner = ui.column {
			id = "panel-empty",
			provide = { panel_key = panel_key },
			w_full = true,
			grow = 1,
			items = "center",
			justify = "center",
			p = "$space.4",
			ui.column {
				w_full = true,
				max_w = 320,
				gap = "$space.1",
				ui.text { "Open a tab", w_full = true, align = "center", mb = "$space.3", text_size = "$font.size.lg", weight = "$font.weight.medium", color = "$text.primary" },
				ui.column {
					hook = "panel.empty",
					w_full = true,
					gap = "$space.1",
					ui.row { id = "panel-open-browser", w_full = true, h = 36, px = "$space.2", items = "center", ui.text { "Browser", text_size = "$font.size.md" } },
				},
			},
		}
	else
		inner = ui.column {
			w_full = true,
			max_w = function(): number
				local now = width:get()
				return if now > 0 then now else 100000
			end,
			h_full = true,
			-- (Changes or History: each its own kind of tab.)
			provide = { tab = (function(): any
				local v = view:peek()
				if v == "commit" then
					-- (A commit's own tab, as History opens one.)
					local c = commit:peek()
					return { kind = "git-commit", title = c and c.subject or "", data = { session = "s1", key = c and `commit:{c.sha}`, commit = c } }
				end
				return { kind = if v == "history" then "git-history" else "git-changes", title = if v == "history" then "History" else "Changes", data = { session = "s1", scope = if scope:peek() ~= "" then scope:peek() else nil } }
			end)() },
			ui.column { id = "slot", class = "plugin-tab plugin-git", w_full = true, h_full = true, ui.text { "placeholder" } },
		}
	end
	return ui.column {
		id = "root",
		w_full = true,
		h_full = true,
		items = "center",
		theme = `zeron-{scheme}`,
		bg = "$zeron.window",
		-- (The chat's panel, as screens/panel provides it to its tabs.)
		provide = { session = "s1", panel = "fixture-panel" },
		inner,
	}
end, { w_full = true, h_full = true })
