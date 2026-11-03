-- A plugin's page's panel and a session's are one width: a plugin's page
-- has no tree, so its panel takes all of the width a session's gives its
-- tree and its tabs, at least the panel's least. (It took a tree's width
-- off there too, opening narrower than a session's, often under its least:
-- screens/shell/page `reserve`.) Opened on either, resized on either, the
-- other follows. On the shell in ui/shell_fixture.luau (`plugin_page`
-- opens a plugin's page; no plugin draws it).

-- The panel's least (screens/shell/fit_policy's RIGHT_PANE_MIN).
local RIGHT_PANE_MIN = 360

local function width(app: any): number
	local line = app:tree("#shell-files"):match("[^\n]+")
	return tonumber(line:match(" %-?%d+,%-?%d+ (%d+)x%d+")) :: number
end
local function route(app: any): string
	return tostring(app:state("shell_route_path"))
end
local function open_panel(app: any)
	app:click('#titlebar >> role:button "Panel"')
	app:frames(60)
end
-- Its edge from the keys (a step each, ⇧ four), as a drag sets it.
local function widen(app: any, steps: number)
	app:click('role:slider "Resize files"')
	app:frames(5)
	for _ = 1, steps do
		app:key("shift+ArrowLeft")
	end
	app:frames(40)
end

test("a plugin's page's panel opens as wide as a session's, and resized on either the other follows", function()
	local app = mount("shell_fixture.luau")
	app:resize(1400, 800)
	app:frames(30)
	-- A session's panel, its tree docked.
	app:click('#shell-sidebar >> role:button "Fix the build"')
	app:frames(20)
	open_panel(app)
	expect(width(app) >= RIGHT_PANE_MIN).to_be(true)

	-- A plugin's page, its own panel opened: at least the panel's least,
	-- and the session's as wide when we go back.
	app:command("plugin_page")
	app:frames(30)
	expect(route(app)).to_be("/plugin/fixture/board")
	open_panel(app)
	local on_page = width(app)
	expect(on_page >= RIGHT_PANE_MIN).to_be(true)
	app:click('#titlebar >> role:button "Back"')
	app:frames(60)
	expect(route(app)).to_be("/session/a1")
	expect(`session {width(app)}`).to_be(`session {on_page}`)

	-- Widened on the plugin's page: the session's follows.
	app:click('#titlebar >> role:button "Forward"')
	app:frames(60)
	expect(route(app)).to_be("/plugin/fixture/board")
	widen(app, 2)
	local widened = width(app)
	expect(widened > on_page).to_be(true)
	app:click('#titlebar >> role:button "Back"')
	app:frames(60)
	expect(`session {width(app)}`).to_be(`session {widened}`)

	-- Widened in the session: the plugin's page's follows.
	widen(app, 1)
	local again = width(app)
	expect(again > widened).to_be(true)
	app:click('#titlebar >> role:button "Forward"')
	app:frames(60)
	expect(`page {width(app)}`).to_be(`page {again}`)
end)

-- Dragged narrow beside its docked tree (the tree floating over the file),
-- then the window narrowed until the fit takes the tree away for room: the
-- panel keeps its least. (It kept only its part beside the tree, a tree's
-- width under its least: screens/shell/page's floor in its resize effect.)
test("a panel narrowed beside its tree never stands under its least as the window narrows", function()
	local app = mount("shell_fixture.luau")
	app:resize(1400, 800)
	app:frames(30)
	app:click('#shell-sidebar >> role:button "Fix the build"')
	app:frames(20)
	open_panel(app)
	-- As narrow as its edge goes.
	app:click('role:slider "Resize files"')
	app:frames(5)
	-- (A step each from where it stands: frames between, as it eases.)
	for _ = 1, 10 do
		app:key("shift+ArrowRight")
		app:frames(20)
	end
	app:frames(40)
	expect(width(app) >= RIGHT_PANE_MIN).to_be(true)
	-- The window narrowed a step at a time, as an edge dragged.
	local w = 1400
	while w > 700 do
		w -= 25
		app:resize(w, 800)
		app:frames(4)
		if app:has("#shell-files") then
			local now = width(app)
			if now > 1 then
				expect(`{w}: {now >= RIGHT_PANE_MIN - 0.5}`).to_be(`{w}: true`)
			end
		end
	end
end)
