-- The right panel in the main window (A's ui/shell_fixture.luau, read as it
-- is): closing a chat's last tab closes that chat's panel, easing shut as
-- the title bar's toggle closes it, and leaves the other chats' alone;
-- opened again, it shows its empty page, its block a little over the
-- panel's middle, level with where a new chat's composer stands.

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

-- Where node `#id` stands in the window: x, y, w, h.
local function box(app: any, id: string): { x: number, y: number, w: number, h: number }?
	for line in app:tree("#shell"):gmatch("[^\n]+") do
		if line:find(` #{id} `, 1, true) or line:sub(-#id - 2) == ` #{id}` then
			local x, y, w, h = line:match(" (%-?%d+),(%-?%d+) (%d+)x(%d+)")
			if x then
				return { x = tonumber(x) :: number, y = tonumber(y) :: number, w = tonumber(w) :: number, h = tonumber(h) :: number }
			end
		end
	end
	return nil
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

test("closing a chat's last tab closes its panel, easing shut; other chats keep theirs; reopened, its empty page, a little over the middle", function()
	local app = mount("shell_fixture.luau")
	app:resize(1200, 800)
	app:frames(30)
	expect(app:error()).to_be_nil()
	-- (Settings → General's "Close the panel with its last tab": on, untouched.)
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
	-- Its block a little over the page's middle (the composer's lift: as
	-- a new chat's composer stands, below).
	expect(eventually(app, function(): boolean
		app:frames(5)
		local page, block = box(app, "panel-empty"), box(app, "panel-home-block")
		if not page or not block or block.h <= 0 or block.h >= page.h then
			return false
		end
		local over, under = block.y - page.y, (page.y + page.h) - (block.y + block.h)
		return over > 24 and under - over > 48
	end)).to_be(true)
	-- A window too short for it: it starts at the page's top and scrolls.
	app:resize(1200, 360)
	expect(eventually(app, function(): boolean
		app:frames(5)
		local page, block = box(app, "panel-empty"), box(app, "panel-home-block")
		return page ~= nil and block ~= nil and block.h > page.h and block.y == page.y
	end)).to_be(true)
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

test("closing a chat's last tab, the panel shuts on what it showed: no frame of its home", function()
	local app = mount("shell_fixture.luau")
	app:resize(1200, 800)
	app:frames(30)
	show_chat(app, "Fix the build")
	open_files_tab(app)
	expect(app:has("#files-pane")).to_be(true)
	app:click("#panel-strip >> role:tab")
	app:key("cmd+w")
	-- Frame by frame as it eases shut: the file tab's view, never the home.
	local seen = 0
	for _ = 1, 120 do
		app:frames(1)
		-- (Shut: the panel itself has no width left, hidden.)
		local p = box(app, "panel")
		if not p or p.w == 0 then
			break
		end
		seen += 1
		expect(app:has("#panel-empty")).to_be(false)
		expect(app:has("#files-pane")).to_be(true)
	end
	expect(seen > 3).to_be(true)
	-- Opened again: its home, at once.
	app:click('#titlebar >> role:button "Panel"')
	expect(app:wait("#panel-empty", { timeout_ms = 15000 })).to_be(true)
	expect(app:has("#panel-strip >> role:tab")).to_be(false)
end)
-- Where node `#id`'s middle stands, down the window.
local function middle(app: any, id: string): number?
	local b = box(app, id)
	return if b and b.h > 0 then b.y + b.h / 2 else nil
end

test("the panel's home stands level with a new chat's composer, its rows no wider than a list's", function()
	local app = mount("shell_fixture.luau")
	for _, size in { { 1100, 760 }, { 1440, 900 }, { 2560, 1440 } } do
		app:resize(size[1], size[2])
		app:frames(30)
		app:command("shell.new_chat")
		app:frames(30)
		if not app:has("#panel-home-block") or (box(app, "panel-home-block") :: any).w == 0 then
			app:click('#titlebar >> role:button "Panel"')
		end
		expect(eventually(app, function(): boolean
			app:frames(5)
			local home, composer = middle(app, "panel-home-block"), middle(app, "chat-composer-dock")
			return home ~= nil and composer ~= nil and math.abs(home - composer) <= 1
		end)).to_be(true)
		-- (No wider than a list's (zeron.form.list), in the panel's middle; each row
		-- as wide as the block's inside, its whole width a target.)
		local panel, block = box(app, "panel-empty") :: any, box(app, "panel-home-block") :: any
		local row = box(app, "panel-open-browser") :: any
		expect(block.w <= 480).to_be(true)
		expect(math.abs((block.x - panel.x) - ((panel.x + panel.w) - (block.x + block.w))) <= 1).to_be(true)
		expect(row.w).to_be(block.w - 32)
		if size[1] == 2560 then
			expect(block.w).to_be(480)
			expect(panel.w > 700).to_be(true)
		end
	end
end)

-- The Files tab's tree as it shows (its width; 0 put away).
local function tree_w(app: any): number
	local b = box(app, "files-tree-beside")
	local pane = box(app, "files-pane")
	if not b or not pane or b.w == 0 then
		return 0
	end
	-- (Slid under the pane's edge when put away.)
	return math.max(0, math.min(b.x + b.w, pane.x + pane.w) - math.max(b.x, pane.x))
end

test("Files with no file and its tree put away shows the tree, never an empty file; a file chosen shows alone", function()
	local app = mount("shell_fixture.luau")
	app:resize(1440, 900)
	app:frames(30)
	show_chat(app, "Fix the build")
	open_files_tab(app)
	expect(tree_w(app) > 200).to_be(true)
	-- Its toggle puts the tree away: with no file to show, the tree stays,
	-- every frame.
	app:click('#files-pane >> role:button "Toggle File Tree"')
	for _ = 1, 30 do
		app:frames(1)
		expect(tree_w(app) > 200).to_be(true)
	end
	-- A file chosen: the file, the tree away as asked.
	app:click('#files-pane >> role:treeitem "README.md"')
	expect(app:wait('#file-viewer-head >> "README.md"', { timeout_ms = 15000 })).to_be(true)
	expect(eventually(app, function(): boolean
		app:frames(5)
		return tree_w(app) == 0
	end)).to_be(true)
end)

test("switching the panel's tabs, Files or not, and its tree in or out, never moves the panel's width, wide or narrow", function()
	for _, size in { { 1440, 900 }, { 1100, 760 } } do
		local app = mount("shell_fixture.luau")
		app:resize(size[1], size[2])
		app:frames(30)
		show_chat(app, "Fix the build")
		open_files_tab(app)
		local w = panel_width(app)
		expect(w > 300).to_be(true)
		local function held(what: string, act: () -> ())
			act()
			for f = 1, 30 do
				app:frames(1)
				expect(`{size[1]}, {what}, frame {f}: {panel_width(app)}`).to_be(`{size[1]}, {what}, frame {f}: {w}`)
			end
		end
		held("a Browser tab", function()
			app:click('#panel-strip >> role:button "New Tab"')
			app:frames(2)
			app:click("#panel-open-browser")
		end)
		held("back to Files", function()
			app:click('#panel-strip >> role:tab "Files"')
		end)
		held("its tree toggled", function()
			app:click('#files-pane >> role:button "Toggle File Tree"')
		end)
		held("its tree toggled back", function()
			app:click('#files-pane >> role:button "Toggle File Tree"')
		end)
		expect(app:error()).to_be_nil()
	end
end)
