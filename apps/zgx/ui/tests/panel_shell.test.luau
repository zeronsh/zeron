-- The right panel in the main window (A's ui/shell_fixture.luau, read as it
-- is): closing a chat's last tab closes that chat's panel, easing shut as
-- the title bar's toggle closes it, and leaves the other chats' alone;
-- opened again, it shows its empty page.

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

-- Whether chat `id`'s panel is open (the shell's own state).
local function panel_open(app: any, id: string): boolean
	return ((app:state("shell_panel_open") :: any) or {})[id] == true
end

-- How wide the panel's column stands now.
local function panel_width(app: any): number
	for line in app:tree("#shell"):gmatch("[^\n]+") do
		if line:find(" #shell-files", 1, true) then
			return tonumber(line:match(" %-?%d+,%-?%d+ (%d+)x%d+")) or 0
		end
	end
	return 0
end

local function show_chat(app: any, title: string)
	app:click(`#shell-sidebar >> role:button "{title}"`)
	app:frames(20)
end

-- Its panel open with a file viewer's tab in it.
local function open_files_tab(app: any)
	app:click('#titlebar >> role:button "Panel"')
	expect(app:wait("#panel-open-files", { timeout_ms = 15000 })).to_be(true)
	-- (Opened all the way: it eases in.)
	local last = -1
	expect(eventually(app, function(): boolean
		app:frames(5)
		local w = panel_width(app)
		local still = w > 300 and w == last
		last = w
		return still
	end)).to_be(true)
	app:click("#panel-open-files")
	expect(app:wait("#panel-strip >> role:tab", { timeout_ms = 15000 })).to_be(true)
	app:frames(30)
end

test("closing a chat's last tab closes its panel, easing shut; other chats keep theirs; reopened, its empty page", function()
	local app = mount("shell_fixture.luau")
	app:resize(1200, 800)
	app:frames(30)
	expect(app:error()).to_be_nil()
	-- (Settings → General's "Close the panel with its last tab", on.)
	app:set_state("settings_close_panel_with_last_tab", true)
	-- Two chats, each with its panel open and a tab in it.
	show_chat(app, "Write the docs")
	open_files_tab(app)
	show_chat(app, "Fix the build")
	open_files_tab(app)
	expect(panel_open(app, "a1")).to_be(true)
	expect(panel_open(app, "a2")).to_be(true)
	local open_w = panel_width(app)
	expect(open_w > 300).to_be(true)
	-- Its one tab closed: the panel closes for this chat, easing (part way
	-- a few frames in, as the toggle's close is).
	app:click("#panel-strip >> role:tab")
	app:key("cmd+w")
	expect(eventually(app, function(): boolean
		return not panel_open(app, "a1")
	end)).to_be(true)
	app:frames(3)
	local mid = panel_width(app)
	expect(mid > 0 and mid < open_w).to_be(true)
	expect(eventually(app, function(): boolean
		app:frames(5)
		return panel_width(app) == 0
	end)).to_be(true)
	-- The other chat's is as it was: open, its tab there.
	expect(panel_open(app, "a2")).to_be(true)
	show_chat(app, "Write the docs")
	expect(eventually(app, function(): boolean
		app:frames(5)
		return panel_width(app) == open_w and app:has("#panel-strip >> role:tab")
	end)).to_be(true)
	-- Back, and opened again: its empty page.
	show_chat(app, "Fix the build")
	app:click('#titlebar >> role:button "Panel"')
	expect(app:wait("#panel-empty", { timeout_ms = 15000 })).to_be(true)
	expect(app:has("#panel-strip >> role:tab")).to_be(false)
end)

-- The focused node's line, as the tree says.
local function focused(app: any): string
	for line in app:tree():gmatch("[^\n]+") do
		if line:find(" focused", 1, true) then
			return line
		end
	end
	return ""
end

-- Tab `n` times from where focus is: whether any stop is in the panel
-- (its strip's tabs, its empty page's choices).
local function tab_reaches_panel(app: any, n: number): boolean
	for _ = 1, n do
		app:key("Tab")
		app:frames(4)
		local line = focused(app)
		if line:find("Panel tabs", 1, true) or line:find("panel-open-files", 1, true) or line:find("Open files", 1, true) then
			return true
		end
	end
	return false
end

test("a closed panel leaves nothing to Tab to: its tabs and choices come back as it opens", function()
	local app = mount("shell_fixture.luau")
	app:resize(1200, 800)
	app:frames(30)
	show_chat(app, "Fix the build")
	expect(panel_open(app, "a1")).to_be(false)
	-- (Closed: none of its strip or its empty page's choices to reach by
	-- Tab. What it holds stays built, hidden, so opening it again shows it
	-- as it was at once: a plugin's page in it isn't read or fetched again.)
	expect(eventually(app, function(): boolean
		app:frames(5)
		return panel_width(app) == 0
	end)).to_be(true)
	expect(tab_reaches_panel(app, 60)).to_be(false)
	app:click('#titlebar >> role:button "Panel"')
	expect(app:wait("#panel-open-files", { timeout_ms = 15000 })).to_be(true)
	expect(app:has("#panel-strip")).to_be(true)
	app:frames(60)
	expect(tab_reaches_panel(app, 80)).to_be(true)
	-- Closed again, once it has eased shut: out of reach again.
	app:click('#titlebar >> role:button "Panel"')
	expect(eventually(app, function(): boolean
		app:frames(5)
		return panel_width(app) == 0
	end)).to_be(true)
	expect(tab_reaches_panel(app, 60)).to_be(false)
end)
