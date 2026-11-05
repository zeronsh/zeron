-- The right panel's home (its empty page) in the main window (A's
-- ui/shell_fixture.luau, read as it is): its letter keys from where the
-- keyboard really is, with no text field holding it; never from the
-- composer's typing.

local function eventually(app: any, ok: () -> boolean, ms: number?): boolean
	local tries = math.ceil((ms or 15000) / 50)
	for _ = 1, tries do
		if ok() then
			return true
		end
		app:wait('"never shown, only waited past"', { timeout_ms = 50 })
	end
	return ok()
end

-- Whether the dialog titled `title` has closed: gone, or kept but hidden.
-- ds.Dialog keeps its modal open through its exit (130 ms), and an open
-- modal takes every click over the window, so a click straight after Save
-- waits for it, as a person's would. (Not a zgx bug: a modal closed at once
-- stops taking clicks at once. `app:wait(t, { gone = true })` can't tell,
-- as a closed modal's kept content still matches; the tree marks it hidden.)
local function closed(app: any, title: string): boolean
	for line in app:tree():gmatch("[^\n]+") do
		if line:find(`dialog "{title}"`, 1, true) then
			return line:find(" hidden", 1, true) ~= nil
		end
	end
	return true
end

-- The kinds of chat `id`'s panel tabs, in order ("terminal,files").
local function kinds(app: any, id: string): string
	local s = ((app:state("panel_sessions") :: any) or {})[`chat:{id}`]
	local out: { string } = {}
	for _, t in (if s then s.tabs else {}) do
		table.insert(out, t.kind)
	end
	return table.concat(out, ",")
end

local function show_chat(app: any, title: string)
	app:click(`#shell-sidebar >> role:button "{title}"`)
	app:frames(20)
end

local function open_home(app: any)
	app:click('#titlebar >> role:button "Panel"')
	expect(app:wait("#panel-empty", { timeout_ms = 15000 })).to_be(true)
	app:frames(30)
end

test("the home's letter keys work from the title bar's Panel button, as the panel opens", function()
	local app = mount("shell_fixture.luau")
	app:resize(1200, 800)
	app:frames(30)
	show_chat(app, "Fix the build")
	open_home(app)
	app:key("t")
	expect(eventually(app, function(): boolean
		return kinds(app, "a1") == "terminal"
	end)).to_be(true)
end)

test("the home's letter keys work from + in the strip", function()
	local app = mount("shell_fixture.luau")
	app:resize(1200, 800)
	app:frames(30)
	show_chat(app, "Fix the build")
	open_home(app)
	app:click("#panel-open-files")
	expect(app:wait('#panel-strip >> role:tab', { timeout_ms = 15000 })).to_be(true)
	app:click('role:button "New Tab"')
	expect(app:wait("#panel-empty", { timeout_ms = 15000 })).to_be(true)
	app:key("b")
	expect(eventually(app, function(): boolean
		return kinds(app, "a1") == "files,browser"
	end)).to_be(true)
end)

test("typing in the composer never opens anything from the home", function()
	local app = mount("shell_fixture.luau")
	app:resize(1200, 800)
	app:frames(30)
	show_chat(app, "Fix the build")
	open_home(app)
	app:type('role:textbox "Message"', "bft")
	app:frames(20)
	expect(kinds(app, "a1")).to_be("")
	expect(app:text('role:textbox "Message"')).to_contain("bft")
end)

-- ---------------------------------------------------------------------------
-- The home itself, on ui/panel_home_fixture.luau.

local function home(): any
	local app = mount("panel_home_fixture.luau")
	app:resize(1000, 900)
	app:frames(20)
	expect(app:error()).to_be_nil()
	expect(app:wait("#panel-empty", { timeout_ms = 15000 })).to_be(true)
	app:frames(10)
	return app
end

local function calls(app: any): string
	return tostring(app:state("calls") or "")
end

local function tabs_of(app: any, id: string?): { any }
	local s = ((app:state("panel_sessions") :: any) or {})[`chat:{id or "chat-1"}`]
	return if s then s.tabs else {}
end

local function home_kinds(app: any): string
	local out: { string } = {}
	for _, t in tabs_of(app) do
		table.insert(out, t.kind)
	end
	return table.concat(out, ",")
end

-- Back to the home ("+"), its rows there again.
local function back_home(app: any)
	app:click('role:button "New Tab"')
	expect(app:wait("#panel-empty", { timeout_ms = 15000 })).to_be(true)
	app:frames(5)
end

test("the home says where the session runs: its device, project and branch, by name alone", function()
	local app = home()
	expect(app:has('#panel-home-device >> \"work-laptop\"')).to_be(true)
	expect(app:has('#panel-home-project >> \"comet-native\"')).to_be(true)
	expect(app:has('#panel-home-branch >> \"zeron/panel-home\"')).to_be(true)
	-- What the branch adds is a plugin's to say (Git's, in the row's
	-- `zeron.panel.branch` slot: src/plugins/examples_tests.rs); with none,
	-- the name alone, and nothing of git asked by core.
	app:frames(10)
	expect(app:has('#panel-home-branch >> "+324962"')).to_be(false)
	expect(app:has("#panel-home-branch-slot")).to_be(true)
	expect(string.find(calls(app), "totals:", 1, true)).to_be(nil)
end)

test("another device's session: its name, away when it can't be reached; no project says so", function()
	local app = home()
	app:set_state("test_home_session", { id = "chat-2", cwd = "/srv" })
	expect(app:wait('#panel-home-device >> "devbox"', { timeout_ms = 15000 })).to_be(true)
	expect(app:has('#panel-home-project >> \"No project\"')).to_be(true)
	expect(app:has('#panel-home-device >> role:img "Away: it can\'t be reached now"')).to_be(false)
	app:command("away")
	app:frames(10)
	expect(app:has('#panel-home-device >> role:img "Away: it can\'t be reached now"')).to_be(true)
	-- (No project: no actions.)
	expect(app:has("#panel-home-actions")).to_be(false)
end)

test("each row opens what it says: browser, file viewer, terminal, side chat, fork", function()
	local app = home()
	app:click("#panel-open-browser")
	expect(eventually(app, function(): boolean
		return home_kinds(app) == "browser"
	end)).to_be(true)
	back_home(app)
	app:click("#panel-open-files")
	expect(eventually(app, function(): boolean
		return home_kinds(app) == "browser,files"
	end)).to_be(true)
	back_home(app)
	app:click("#panel-open-terminal")
	expect(eventually(app, function(): boolean
		return home_kinds(app) == "browser,files,terminal"
	end)).to_be(true)
	back_home(app)
	app:click("#panel-open-side-chat")
	expect(eventually(app, function(): boolean
		return home_kinds(app) == "browser,files,terminal,side_chat" and app:has(".side-chat-tab")
	end)).to_be(true)
	-- (Made by its first message, not before.)
	expect(calls(app)).never.to_contain("create:")
	back_home(app)
	app:click("#panel-open-fork")
	expect(eventually(app, function(): boolean
		return home_kinds(app) == "browser,files,terminal,side_chat,side_chat" and calls(app):find("fork:chat-1:", 1, true) ~= nil
	end)).to_be(true)
end)

test("the letters work from a button outside the panel, and S and K make a side chat and a fork", function()
	local app = home()
	expect(app:has('#panel-open-side-chat >> "S"')).to_be(true)
	expect(app:has('#panel-open-fork >> "K"')).to_be(true)
	app:click("#outside")
	app:key("s")
	expect(eventually(app, function(): boolean
		return home_kinds(app) == "side_chat"
	end)).to_be(true)
	back_home(app)
	app:click("#outside")
	app:key("k")
	expect(eventually(app, function(): boolean
		return calls(app):find("fork:chat-1:", 1, true) ~= nil and home_kinds(app) == "side_chat,side_chat"
	end)).to_be(true)
	back_home(app)
	-- A text field keeps its typing.
	app:type('role:textbox "Message"', "tfb")
	app:frames(10)
	expect(home_kinds(app)).to_be("side_chat,side_chat")
end)

test("a letter the home has no row for is passed on to plugins' rows; none from a text field", function()
	local app = home()
	app:click("#outside")
	app:key("c")
	app:frames(3)
	local passed = app:state("panel_home_key") :: any
	expect(passed.key).to_be("C")
	local n = passed.n
	app:key("h")
	app:frames(3)
	passed = app:state("panel_home_key") :: any
	expect(passed.key).to_be("H")
	expect(passed.n).to_be(n + 1)
	app:type('role:textbox "Message"', "c")
	app:frames(3)
	expect((app:state("panel_home_key") :: any).n).to_be(n + 1)
end)

test("an action runs in a terminal tab of its own, titled by it, attached to the engine's terminal", function()
	local app = home()
	expect(app:wait('role:button "Run Dev server"', { timeout_ms = 15000 })).to_be(true)
	expect(calls(app)).to_contain("actions:p1:d")
	app:click('role:button "Run Dev server"')
	expect(eventually(app, function(): boolean
		return app:has("#stub-terminal") and app:text("#stub-terminal"):find("bridge term-action", 1, true) ~= nil
	end)).to_be(true)
	expect(calls(app)).to_contain("run:p1:chat-1:d:a1")
	expect(calls(app)).to_contain("term:attach:term-action")
	expect(app:has('#panel-strip >> role:tab "Dev server"')).to_be(true)
end)

test("Add action asks for a name and a command, then adds it; an import comes filled in", function()
	local app = home()
	expect(app:wait('role:button "Add action"', { timeout_ms = 15000 })).to_be(true)
	app:click('role:button "Add action"')
	expect(app:wait('role:dialog >> "Add action"', { timeout_ms = 15000 })).to_be(true)
	app:frames(30)
	app:click('role:button "Save action"')
	app:frames(5)
	expect(app:has('#panel-home-editor-problem >> \"Action name is required\"')).to_be(true)
	app:type('role:textbox "Name"', "Lint")
	app:click('role:button "Save action"')
	app:frames(5)
	expect(app:has('#panel-home-editor-problem >> \"Action command is required\"')).to_be(true)
	app:type('role:textbox "Command"', "cargo clippy")
	app:click('role:radio "Lint"')
	app:click('role:checkbox "Run automatically on worktree creation"')
	app:click('role:button "Save action"')
	expect(eventually(app, function(): boolean
		return app:has('role:button "Run Lint (setup)"') or app:has('role:button "Run Lint"')
	end)).to_be(true)
	expect(calls(app)).to_contain("save:p1:new:Lint:cargo clippy:lint:true")
	expect(app:has('#panel-home-actions >> "Lint (setup)"')).to_be(true)
	expect(eventually(app, function(): boolean
		return closed(app, "Add action")
	end)).to_be(true)
	-- An import from zeron.json: the editor, filled in from it.
	app:click('role:button "Import Tests"')
	expect(app:wait('role:dialog >> "Add action"', { timeout_ms = 15000 })).to_be(true)
	app:frames(30)
	app:click('role:button "Save action"')
	expect(eventually(app, function(): boolean
		return calls(app):find("save:p1:new:Tests:cargo test:test:false", 1, true) ~= nil
	end)).to_be(true)
end)

test("an action's gear edits it; Delete asks first, then it's gone", function()
	local app = home()
	expect(app:wait('role:button "Run Dev server"', { timeout_ms = 15000 })).to_be(true)
	-- (Its gear shows under the pointer, as the shipping app's.)
	local function gear_line(): string
		for line in app:tree():gmatch("[^\n]+") do
			if line:find('button "Edit Dev server"', 1, true) then
				return line
			end
		end
		return ""
	end
	expect(gear_line()).to_contain("hidden")
	app:hover('role:button "Run Dev server"')
	app:frames(5)
	expect(gear_line()).never.to_contain("hidden")
	app:click('role:button "Edit Dev server"')
	expect(app:wait('role:dialog >> "Edit action"', { timeout_ms = 15000 })).to_be(true)
	app:frames(30)
	expect(app:text('role:textbox "Name"')).to_contain("Dev server")
	-- (Editing never runs it.)
	expect(calls(app)).never.to_contain("run:")
	app:type('role:textbox "Name"', " 2")
	app:click('role:button "Save action"')
	expect(eventually(app, function(): boolean
		return app:has('role:button "Run Dev server 2"')
	end)).to_be(true)
	expect(calls(app)).to_contain("save:p1:a1:Dev server 2:npm run dev:play:false")
	expect(eventually(app, function(): boolean
		return closed(app, "Edit action")
	end)).to_be(true)
	app:hover('role:button "Run Dev server 2"')
	app:frames(5)
	app:click('role:button "Edit Dev server 2"')
	expect(app:wait('role:button "Delete action"', { timeout_ms = 15000 })).to_be(true)
	app:frames(30)
	app:click('role:button "Delete action"')
	expect(app:wait('"Delete action?"', { timeout_ms = 15000 })).to_be(true)
	app:frames(30)
	app:click('role:button "Delete"')
	expect(eventually(app, function(): boolean
		return not app:has('role:button "Run Dev server 2"')
	end)).to_be(true)
	expect(calls(app)).to_contain("delete:p1:a1")
end)

test("actions that couldn't load say why, and Retry loads them", function()
	local app = home()
	-- (Asked again from nothing, as a new window does, while the engine
	-- can't.)
	app:command("actions_fail")
	app:set_state("panel_home_actions", {})
	app:set_state("test_home_generation", 2)
	app:frames(20)
	expect(app:wait('"The engine couldn\'t read zeron.json"', { timeout_ms = 15000 })).to_be(true)
	app:command("actions_ok")
	app:click("#panel-home-actions-retry")
	expect(app:wait('role:button "Run Dev server"', { timeout_ms = 15000 })).to_be(true)
end)

-- How many of a list's rows are built now.
local function built(app: any, what: string): number
	local n = 0
	for _ in app:tree():gmatch(what) do
		n += 1
	end
	return n
end

test("the side chats and subagents lists open their lanes' tabs", function()
	local app = home()
	expect(app:has('#panel-home-side >> "Side chats"')).to_be(true)
	expect(app:has('#panel-home-subagents >> "Subagents"')).to_be(true)
	app:click('role:button "Open side chat Why does the sync test flake?"')
	expect(eventually(app, function(): boolean
		return app:has(".side-chat-tab") and calls(app):find("watch:side-a", 1, true) ~= nil
	end)).to_be(true)
	back_home(app)
	app:click('role:button "Open subagent scan the repo"')
	expect(eventually(app, function(): boolean
		return app:has(".subagent-tab") and calls(app):find("watch:chat-1--sub--1", 1, true) ~= nil
	end)).to_be(true)
	back_home(app)
	-- A finished one is read from its upload.
	app:click('role:button "Open subagent read the docs"')
	expect(eventually(app, function(): boolean
		return calls(app):find("watch:chat-1--sub--2:frozen", 1, true) ~= nil
	end)).to_be(true)
end)

test("hundreds of lanes: the first five and those needing you, then Show N more; shown whole, the home scrolls to the last", function()
	local app = home()
	app:command("many")
	app:frames(30)
	-- (Folded, a part says how many it holds, as the sidebar's groups do.)
	app:click("#panel-home-side-head")
	app:frames(30)
	app:click("#panel-home-subagents-head")
	app:frames(30)
	expect(app:has('#panel-home-side-head >> "302"')).to_be(true)
	expect(app:has('#panel-home-subagents-head >> "202"')).to_be(true)
	app:click("#panel-home-subagents-head")
	app:frames(30)
	app:click("#panel-home-side-head")
	app:frames(30)
	-- Five, and the one far down that waits on you.
	expect(built(app, 'button "Open side chat ')).to_be(6)
	expect(app:has('role:button "Open side chat Side chat 250"')).to_be(true)
	expect(app:has('role:button "Open side chat Side chat 4"')).to_be(false)
	expect(app:has('#panel-home-side-list-pager >> "Show 296 more"')).to_be(true)
	-- (Finished and unseen, they'd all show as those needing you do: seen,
	-- their first five.)
	expect(built(app, 'button "Open subagent ') > 5).to_be(true)
	local seen = {}
	for i = 2, 202 do
		seen[`chat-1--sub--{i}`] = os.time()
	end
	app:set_state("screens/panel/lanes_seen.luau::panel_lanes_seen", seen)
	app:frames(30)
	expect(built(app, 'button "Open subagent ')).to_be(5)
	expect(app:has('#panel-home-subagent-list-pager >> "Show 197 more"')).to_be(true)
	-- Shown whole: every row of the group, the list as tall as they are,
	-- and the home is what scrolls to reach the last of them (no list
	-- scrolls inside the panel). (Only the rows in the home's view will be
	-- built once zgx's ui.list builds against its scrolling ancestor:
	-- zgx-asks/list-ancestor-viewport.)
	app:scroll("#panel-empty", 1200)
	app:frames(5)
	app:click("#panel-home-side-list-pager")
	app:frames(30)
	expect(app:has('#panel-home-side-list-pager >> "Show less"')).to_be(true)
	-- (Down the home to its end, where the last rows stand.)
	for _ = 1, 40 do
		app:scroll("#panel-empty", 600)
		app:frames(2)
	end
	expect(app:has('role:button "Open side chat Side chat 300"')).to_be(true)
	app:click('role:button "Open side chat Side chat 300"')
	expect(eventually(app, function(): boolean
		return calls(app):find("watch:side-300", 1, true) ~= nil
	end)).to_be(true)
	-- Back home, Show less (under the last row, at the home's end): the
	-- first few again.
	back_home(app)
	for _ = 1, 40 do
		app:scroll("#panel-empty", 600)
		app:frames(2)
	end
	app:click("#panel-home-side-list-pager")
	app:frames(30)
	expect(built(app, 'button "Open side chat ')).to_be(6)
end)

test("a part folded stays folded for its session, across sessions and a reload; another's is its own", function()
	local app = home()
	app:click("#panel-home-side-head")
	-- (It eases shut, as the sidebar's groups do, then its rows are gone.)
	app:frames(30)
	expect(app:has('role:button "Open side chat Why does the sync test flake?"')).to_be(false)
	expect(app:has('role:button "Open subagent scan the repo"')).to_be(true)
	-- Another session and back: still folded.
	app:set_state("test_home_session", { id = "chat-2", cwd = "/srv" })
	expect(app:wait('#panel-home-device >> "devbox"', { timeout_ms = 15000 })).to_be(true)
	app:set_state("test_home_session", { id = "chat-1", cwd = "/proj" })
	expect(app:wait('#panel-home-device >> "work-laptop"', { timeout_ms = 15000 })).to_be(true)
	expect(app:has('role:button "Open side chat Why does the sync test flake?"')).to_be(false)
	-- Made again, as a reload does.
	app:set_state("test_home_generation", 2)
	expect(app:wait('#panel-home-side-head', { timeout_ms = 15000 })).to_be(true)
	app:frames(10)
	expect(app:has('role:button "Open side chat Why does the sync test flake?"')).to_be(false)
	app:click("#panel-home-side-head")
	app:frames(10)
	expect(app:has('role:button "Open side chat Why does the sync test flake?"')).to_be(true)
end)

-- Where a node's top is in the window.
local function top_of(app: any, target: string): number?
	for line in app:tree(target):gmatch("[^\n]+") do
		local y = line:match(" %-?%d+,(%-?%d+) %d+x%d+")
		if y then
			return tonumber(y)
		end
	end
	return nil
end

test("the home stands near the panel's top and stays put as its parts open, close and grow", function()
	local app = home()
	local y0 = top_of(app, "#panel-home-device") :: number
	-- Near the panel's top, just under the strip (a small share of the
	-- panel's height, 16–48): the home holds a lot now.
	expect(y0 > 40 and y0 < 160).to_be(true)
	app:click("#panel-home-subagents-head")
	app:frames(30)
	expect(top_of(app, "#panel-home-device")).to_be(y0)
	app:click("#panel-home-subagents-head")
	app:command("many")
	app:frames(30)
	expect(top_of(app, "#panel-home-device")).to_be(y0)
end)

-- Where a node's left edge is in the window.
local function left_of(app: any, target: string): number?
	for line in app:tree(target):gmatch("[^\n]+") do
		local x = line:match(" (%-?%d+),%-?%d+ %d+x%d+")
		if x then
			return tonumber(x)
		end
	end
	return nil
end

test("side chats and subagents are the sidebar's rows: its header, its pager, its view's Harness", function()
	local app = home()
	-- The sidebar's own pieces (ds/session_row): a group's header, a
	-- session's row with its title, off the group's trunk.
	expect(app:has("#panel-home-side-head")).to_be(true)
	expect(app:has("#panel-home-side-list >> .panel-home-lane >> .session-title")).to_be(true)
	local with_mark = left_of(app, '#panel-home-side-list >> "Why does the sync test flake?"') :: number
	-- The sidebar's view without the harness's mark: the title moves in to
	-- where the mark stood.
	app:set_state("sidebar.view", { details = { harness = false } })
	app:frames(30)
	local without = left_of(app, '#panel-home-side-list >> "Why does the sync test flake?"') :: number
	expect(without < with_mark).to_be(true)
	app:set_state("sidebar.view", { details = { harness = true } })
	app:frames(30)
	expect(left_of(app, '#panel-home-side-list >> "Why does the sync test flake?"')).to_be(with_mark)
end)

-- What a lane's row says, its corner included ("Completed", a time).
local function lane_row(app: any, label: string): string
	return app:tree(`role:button "{label}"`)
end

test("a finished lane shows its check until its tab is opened, then its time; finishing again shows it again", function()
	local app = home()
	-- A finished subagent, never opened: its check.
	expect(lane_row(app, "Open subagent read the docs")).to_contain("Completed")
	app:click('role:button "Open subagent read the docs"')
	expect(eventually(app, function(): boolean
		return calls(app):find("watch:chat-1--sub--2:frozen", 1, true) ~= nil
	end)).to_be(true)
	back_home(app)
	-- Seen: its time.
	expect(lane_row(app, "Open subagent read the docs")).never.to_contain("Completed")
	expect(lane_row(app, "Open subagent read the docs")).to_contain("5m")

	-- A side chat finishes: its check; opened, gone.
	app:command("side_done")
	app:frames(10)
	expect(lane_row(app, "Open side chat Draft the release notes")).to_contain("Completed")
	app:click('role:button "Open side chat Draft the release notes"')
	expect(app:wait(".side-chat-tab", { timeout_ms = 15000 })).to_be(true)
	back_home(app)
	expect(lane_row(app, "Open side chat Draft the release notes")).never.to_contain("Completed")
	-- A new reply finishes it again, later than it was seen: the check again.
	app:command("side_done")
	app:frames(10)
	expect(lane_row(app, "Open side chat Draft the release notes")).to_contain("Completed")
	-- (Kept across launches, by lane.)
	local kept = (app:state("screens/panel/lanes_seen.luau::panel_lanes_seen") :: any) or {}
	expect(kept["chat-1--sub--2"] ~= nil).to_be(true)
end)

test("unseen finished lanes count as those needing you: they show past the first five", function()
	local app = home()
	app:command("many")
	app:frames(30)
	-- The 200 finished subagents, unseen: none hidden behind Show more.
	expect(app:has('#panel-home-subagent-list-pager')).to_be(false)
end)

-- The group headers a part shows (their names, in order).
local function groups(app: any, list: string): { string }
	local out = {}
	for line in app:tree(list):gmatch("[^\n]+") do
		local name = line:match(' button "([^"]+)"')
		if name and not name:find("^Open ") and not name:find("^Show ") then
			table.insert(out, name)
		end
	end
	return out
end

test("side chats are grouped as the sidebar's view groups sessions (by project, by device, or one list); subagents never are", function()
	local app = home()
	app:command("side_elsewhere")
	app:frames(10)
	-- One list (the sidebar's "None"): no group headers.
	expect(#groups(app, "#panel-home-side-list")).to_be(0)
	-- By project: the session's project, its tile and name, over all of
	-- them (a side chat is its session's project's, wherever it runs).
	app:set_state("sidebar.view", { organize = "project" })
	app:frames(30)
	expect(table.concat(groups(app, "#panel-home-side-list"), ",")).to_be("comet-native")
	-- (Subagents never group: they're always the session's, one list.)
	expect(#groups(app, "#panel-home-subagent-list")).to_be(0)
	-- By device: the devbox's side chat in a group of its own, the newest
	-- first, as the sidebar orders its groups.
	app:set_state("sidebar.view", { organize = "device" })
	app:frames(30)
	expect(table.concat(groups(app, "#panel-home-side-list"), ",")).to_be("devbox,work-laptop")
	expect(#groups(app, "#panel-home-subagent-list")).to_be(0)
	expect(app:has('#panel-home-subagent-list >> role:button "Open subagent scan the repo"')).to_be(true)
	-- A group folds on its header, as the sidebar's do; the rest stay.
	app:click('#panel-home-side-list >> role:button "devbox"')
	app:frames(30)
	expect(app:has('role:button "Open side chat Try it on the devbox"')).to_be(false)
	expect(app:has('role:button "Open side chat Draft the release notes"')).to_be(true)
end)

test("a lane's tab in front from anywhere (a spawn chip, the lanes card) counts as seeing it", function()
	local app = home()
	expect(lane_row(app, "Open subagent read the docs")).to_contain("Completed")
	-- Its tab made and brought to the front, as a spawn chip's click does,
	-- not from the home.
	app:set_state("panel_sessions", {
		["chat:chat-1"] = { tabs = { { id = "chip-tab", kind = "subagent", chat = "chat-1--sub--2", title = "read the docs" } }, active = "chip-tab", visited = { "chip-tab" } },
	})
	app:frames(10)
	local kept = (app:state("screens/panel/lanes_seen.luau::panel_lanes_seen") :: any) or {}
	expect(kept["chat-1--sub--2"] ~= nil).to_be(true)
end)

-- A row's height in the window, by its label.
local function row_h(app: any, scope: string, label: string): number?
	for line in app:tree(scope):gmatch("[^\n]+") do
		if line:find(` button "{label}"`, 1, true) then
			return tonumber(line:match(" %-?%d+,%-?%d+ %d+x(%d+)"))
		end
	end
	return nil
end

test("side chats follow the sidebar's density: compact rows as the sidebar's, regular with their second line", function()
	-- The sidebar's own rows, compact and regular (the shell's fixture).
	local shell = mount("shell_fixture.luau")
	shell:resize(1200, 800)
	shell:frames(30)
	local sidebar_compact = row_h(shell, "#shell-sidebar", "Fix the build") :: number
	shell:set_state("screens/shell/sidebar.luau::shell_sidebar_compact", false)
	shell:frames(40)
	local sidebar_regular = row_h(shell, "#shell-sidebar", "Fix the build") :: number
	expect(sidebar_regular > sidebar_compact).to_be(true)

	-- The panel's side chats, as the sidebar's view says (its value as the
	-- shell publishes it), switched live.
	local app = home()
	app:set_state("sidebar.view", { compact = true, organize = "project" })
	app:frames(30)
	expect(row_h(app, "#panel-home-side-list", "Open side chat Draft the release notes")).to_be(sidebar_compact)
	app:set_state("sidebar.view", { compact = false, organize = "project" })
	app:frames(40)
	expect(row_h(app, "#panel-home-side-list", "Open side chat Draft the release notes")).to_be(sidebar_regular)
	-- Its second line: where it runs, as the sidebar's says (by project: its
	-- device).
	expect(app:tree('role:button "Open side chat Draft the release notes"')).to_contain("work-laptop")
	-- Location hidden: nothing for a second line, one line again.
	app:set_state("sidebar.view", { compact = false, organize = "project", details = { location = false } })
	app:frames(40)
	expect(row_h(app, "#panel-home-side-list", "Open side chat Draft the release notes")).to_be(sidebar_compact)
	-- Back to compact, live.
	app:set_state("sidebar.view", { compact = true, organize = "project" })
	app:frames(40)
	expect(row_h(app, "#panel-home-side-list", "Open side chat Draft the release notes")).to_be(sidebar_compact)
end)

test("a side chat row's menu is the sidebar's, without pin or move; a top-level chat's keeps them", function()
	local app = home()
	app:set_state("test_menu_of", "side-a")
	app:command("session_menu")
	local side = tostring(app:state("test_menu_labels"))
	for _, item in { "Rename", "Copy", "Archive", "Delete…" } do
		expect(side).to_contain(item)
	end
	expect(side).never.to_contain("Pin")
	expect(side).never.to_contain("Move to")
	app:set_state("test_menu_of", "chat-1")
	app:command("session_menu")
	local top = tostring(app:state("test_menu_labels"))
	expect(top).to_contain("Pin")
	expect(top).to_contain("Move to")
end)

test("a subagent row's menu only opens it", function()
	local app = home()
	app:command("subagent_menu")
	app:frames(20)
	expect(tostring(app:state("test_menu_labels"))).to_be("Open")
	expect(eventually(app, function(): boolean
		return home_kinds(app) == "subagent"
	end)).to_be(true)
end)

-- The host's previews for chat-1: `n` servers (web, then port-2…).
local function ports(app: any, n: number, extra: { [string]: any }?)
	local services = {}
	for i = 1, n do
		local name = if i == 1 then "web" else `port-{i}`
		-- (Their processes long started: open before the home looked, they
		-- show at once.)
		table.insert(services, { id = `svc-{i}`, name = name, port = 2999 + i, url = `http://{name}.proj.localhost:7400`, project = "comet-native", device = "work-laptop", cwd = "/proj", remote = false, started = 1 })
	end
	local p: { [string]: any } = { chat = "chat-1", ready = true, services = services }
	for k, v in extra or {} do
		p[k] = v
	end
	app:set_state("previews", p)
	app:frames(20)
end

test("Ports show only while the session has some; a port opens in a browser tab of the panel", function()
	local app = home()
	expect(app:has("#panel-home-ports")).to_be(false)
	ports(app, 2)
	expect(app:has('#panel-home-ports-head >> "Ports"')).to_be(true)
	expect(app:has('role:button "Open web preview"')).to_be(true)
	expect(app:has('role:button "Open web preview" >> ":3000"')).to_be(true)
	-- Another session's ports aren't this one's.
	app:set_state("previews", { chat = "chat-9", ready = true, services = { { id = "x", name = "other", port = 1, url = "http://x.localhost:7400" } } })
	app:frames(10)
	expect(app:has("#panel-home-ports")).to_be(false)
	ports(app, 2)
	app:click('role:button "Open web preview"')
	expect(eventually(app, function(): boolean
		local t = tabs_of(app)
		return #t == 1 and t[1].kind == "browser" and t[1].url == "http://web.proj.localhost:7400"
	end)).to_be(true)
end)

test("Ports fold, and show their first five then Show N more", function()
	local app = home()
	ports(app, 8)
	expect(app:has('role:button "Open port-5 preview"')).to_be(true)
	expect(app:has('role:button "Open port-6 preview"')).to_be(false)
	app:scroll("#panel-empty", 1200)
	app:frames(5)
	expect(app:has('#panel-home-port-list-pager >> "Show 3 more"')).to_be(true)
	app:click("#panel-home-port-list-pager")
	app:frames(30)
	expect(app:has('role:button "Open port-8 preview"')).to_be(true)
	app:click("#panel-home-ports-head")
	app:frames(30)
	expect(app:has('role:button "Open web preview"')).to_be(false)
	expect(((app:state("panel_home_folds") :: any) or {})["chat-1/ports"]).to_be(true)
end)

test("a port's menu opens it, opens it in the default browser, or copies its address", function()
	local app = home()
	app:command("port_menu")
	expect(tostring(app:state("test_menu_labels"))).to_be("Open,Open in Default Browser,Copy URL")
	app:set_state("test_menu_pick", "Open in Default Browser")
	app:command("port_menu_pick")
	expect(calls(app)).to_contain("external:http://web.proj.localhost:7400")
	app:set_state("test_menu_pick", "Open")
	app:command("port_menu_pick")
	expect(calls(app)).to_contain("port_open")
end)

-- chat-1's previews as the host has them now: `services`, each started
-- just now unless it says.
local function serving(app: any, services: { any })
	app:set_state("previews", { chat = "chat-1", ready = true, services = services })
end

local function test_server(id: string, port: number): any
	return {
		id = id,
		name = "HTTP server",
		port = port,
		url = `http://{id}.proj.localhost:7400`,
		process = "python3 -m http.server",
		command = `python3 -m http.server {port}`,
	}
end

-- (A frame is 1/60 s of the test's clock.)
local function seconds(app: any, s: number)
	app:frames(math.ceil(s * 60))
end

test("a port open under five seconds never shows; one open longer shows, then stays a while once closed", function()
	local app = home()
	serving(app, {})
	seconds(app, 0.5)
	-- A test's server: open a second, gone.
	serving(app, { test_server("blip", 41001) })
	seconds(app, 1)
	expect(app:has("#panel-home-ports")).to_be(false)
	serving(app, {})
	seconds(app, 6)
	expect(app:has("#panel-home-ports")).to_be(false)
	-- A dev server: shows once it's stayed open five seconds.
	serving(app, { test_server("dev", 8000) })
	seconds(app, 4)
	expect(app:has("#panel-home-ports")).to_be(false)
	seconds(app, 1.5)
	expect(app:has('role:button "Open python3 -m http.server preview"')).to_be(true)
	-- Closed (restarting): still listed, ten seconds.
	serving(app, {})
	seconds(app, 8)
	expect(app:has('role:button "Open python3 -m http.server preview"')).to_be(true)
	-- Back in time: listed on, as if it never went.
	serving(app, { test_server("dev", 8000) })
	seconds(app, 0.5)
	serving(app, {})
	seconds(app, 9)
	expect(app:has('role:button "Open python3 -m http.server preview"')).to_be(true)
	seconds(app, 1.5)
	expect(app:has("#panel-home-ports")).to_be(false)
end)

test("a port's row names the process serving it, its command line in its tooltip", function()
	local app = home()
	local sv = test_server("dev", 8000)
	sv.started = 1
	serving(app, { sv })
	app:frames(20)
	expect(app:has('role:button "Open python3 -m http.server preview" >> ":8000"')).to_be(true)
	expect(app:has('"HTTP server"')).to_be(false)
	app:hover('role:button "Open python3 -m http.server preview"')
	expect(app:wait('"python3 -m http.server 8000"', { timeout_ms = 5000 })).to_be(true)
	-- Without one (an engine that names none, a remote device's): what the
	-- engine calls it.
	serving(app, { { id = "web", name = "Vite", port = 5173, url = "http://web.proj.localhost:7400", started = 1 } })
	seconds(app, 0.5)
	-- (Not the first answer: it settles first.)
	seconds(app, 5.5)
	expect(app:has('role:button "Open Vite preview"')).to_be(true)
end)

test("local previews that can't open say so in one plain line, never the system's words", function()
	local app = home()
	ports(app, 1, { error = "Local previews could not start: Address already in use (os error 48)" })
	expect(app:has('#panel-home-ports-trouble >> "Local previews aren\'t available right now"')).to_be(true)
	expect(app:has('~"os error"')).to_be(false)
	expect(app:has('~"Address already in use"')).to_be(false)
	-- (No ports: nothing to say it about.)
	ports(app, 0, { error = "Local previews could not start: Address already in use (os error 48)" })
	expect(app:has("#panel-home-ports-trouble")).to_be(false)
end)

-- Where `target`'s top is in the window, or nil when it isn't there.
local function y_of(app: any, target: string): number?
	if not app:has(target) then
		return nil
	end
	for line in app:tree(target):gmatch("[^\n]+") do
		local v = line:match(" %-?%d+,(%-?%d+) %d+x%d+")
		if v then
			return tonumber(v)
		end
	end
	return nil
end

test("the wheel over a part's rows scrolls the home while the part has nothing to scroll", function()
	local app = home()
	-- (Short enough that the home scrolls, tall enough that each part's
	-- rows are in view.)
	app:resize(1000, 760)
	app:frames(20)
	ports(app, 2)
	-- Over a side chat's row, a subagent's, a port's, and a part's title:
	-- the home moves each time.
	for _, over in {
		'#panel-home-side-list >> role:button "Open side chat Draft the release notes"',
		'#panel-home-subagent-list >> role:button "Open subagent scan the repo"',
		'role:button "Open web preview"',
		"#panel-home-side-head",
	} do
		-- (The page at its top, then the row brought to the window's
		-- middle, so the home can scroll on both ways.)
		app:scroll("#panel-empty", -3000)
		app:frames(10)
		app:wait('"never shown, only waited past"', { timeout_ms = 700 })
		local at = y_of(app, over) :: number
		if at > 380 then
			app:scroll("#panel-empty", at - 380)
			app:frames(10)
			app:wait('"never shown, only waited past"', { timeout_ms = 700 })
		end
		-- Down, or (the home at its end) up: either way, the home moves.
		local before = y_of(app, "#panel-home-device") :: number
		app:scroll(over, 150)
		app:frames(10)
		local after = y_of(app, "#panel-home-device") :: number
		if after == before then
			app:wait('"never shown, only waited past"', { timeout_ms = 700 })
			app:scroll(over, -150)
			app:frames(10)
			after = y_of(app, "#panel-home-device") :: number
		end
		expect(`{over}: moved {after ~= before}`).to_be(`{over}: moved true`)
	end
end)

test("a part never scrolls on its own: the wheel over its rows scrolls the home, shown whole or not", function()
	local app = home()
	app:command("many")
	app:frames(30)
	-- (Down to its pager, under its first rows, and Show all.)
	for _ = 1, 20 do
		if pcall(function()
			app:click("#panel-home-side-list-pager")
		end) then
			break
		end
		app:scroll("#panel-empty", 120)
		app:frames(3)
	end
	app:frames(30)
	expect(app:has('#panel-home-side-list-pager >> "Show less"')).to_be(true)
	app:scroll("#panel-empty", -3000)
	app:frames(10)
	app:wait('"never shown, only waited past"', { timeout_ms = 700 })
	local page = y_of(app, "#panel-home-device") :: number
	local first = y_of(app, '#panel-home-side-list >> role:button "Open side chat Side chat 1"') :: number
	-- The wheel over its rows: the whole home moves, the rows with it, by
	-- the same (the list never scrolls on its own).
	app:scroll('#panel-home-side-list >> role:button "Open side chat Side chat 1"', 300)
	app:frames(10)
	local moved = page - (y_of(app, "#panel-home-device") :: number)
	expect(moved > 0).to_be(true)
	expect(first - (y_of(app, '#panel-home-side-list >> role:button "Open side chat Side chat 1"') :: number)).to_be(moved)
end)
