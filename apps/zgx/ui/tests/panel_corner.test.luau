-- The window's top right mirrors its top left: Expand stands off the title
-- bar's panel toggle by the left cluster's gap between the sidebar toggle and
-- Back, level with them, in each state of the panel and the sidebar, and in a
-- panel split into groups (its top right group's Expand). The strip's tabs
-- are on the same line. Expand's arrows reach less far up and down than the
-- toggle's frame, centred in its button: open arrows look taller than a closed
-- frame of the same height. On the shell in ui/shell_fixture.luau.

type Box = { x: number, y: number, w: number, h: number }

local function eventually(app: any, ok: () -> boolean): boolean
	for _ = 1, 300 do
		if ok() then
			return true
		end
		app:wait('"never shown, only waited past"', { timeout_ms = 50 })
	end
	return ok()
end

-- The first node showing in `scope` whose line holds `name`, where it stands.
-- (The one panel's strip stays mounted, of no width, while it's in groups.)
local function box(app: any, scope: string, name: string): Box?
	for line in app:tree(scope):gmatch("[^\n]+") do
		if line:find(name, 1, true) then
			local x, y, w, h = line:match(" (%-?[%d%.]+),(%-?[%d%.]+) ([%d%.]+)x([%d%.]+)")
			if x and (tonumber(w) :: number) > 0 and not line:find(" hidden", 1, true) then
				return { x = tonumber(x) :: number, y = tonumber(y) :: number, w = tonumber(w) :: number, h = tonumber(h) :: number }
			end
		end
	end
	return nil
end

local function must(app: any, scope: string, name: string): Box
	local found = box(app, scope, name)
	if not found then
		error(`{name} isn't in {scope}`)
	end
	return found
end

local function gap(a: Box, b: Box): number
	return b.x - (a.x + a.w)
end

-- How tall the toggle's frame is drawn: its 16pt glyph's 17.75 of 24 units
-- (ds/sidebar_icon), stroke included.
local FRAME_INK = 16 * 17.75 / 24
-- The heaviest glyph stroke, in points (light's, ds/themes/zeron-light.json).
local HEAVIEST_STROKE = 1.45

-- The glyph drawn in the button `button` (the node after it in `scope`'s tree).
local function glyph_in(app: any, scope: string, name: string, button: Box): Box
	local past = false
	for line in app:tree(scope, { all = true }):gmatch("[^\n]+") do
		if past and line:find(" img ", 1, true) then
			local x, y, w, h = line:match(" (%-?[%d%.]+),(%-?[%d%.]+) ([%d%.]+)x([%d%.]+)")
			return { x = tonumber(x) :: number, y = tonumber(y) :: number, w = tonumber(w) :: number, h = tonumber(h) :: number }
		end
		if line:find(name, 1, true) and line:find(` {button.x},{button.y} `, 1, true) then
			past = true
		end
	end
	error(`no glyph in {name}`)
end

-- The two sides mirrored, with `expand` (its name as it stands now) in `scope`.
local function expect_mirrored(app: any, scope: string, expand: string)
	local toggle = must(app, "#titlebar", '"Toggle sidebar"')
	local back = must(app, "#titlebar", '"Back"')
	local panel = must(app, "#titlebar", '"Panel"')
	local arrows = must(app, scope, expand)
	expect(gap(arrows, panel)).to_be(gap(toggle, back))
	expect(arrows.y).to_be(panel.y)
	expect(arrows.y).to_be(toggle.y)
	expect(arrows.w).to_be(panel.w)
	expect(arrows.h).to_be(panel.h)
	-- (The arrows' ink: the site's 16 of 24 units, and the stroke, the same
	-- points at any size.)
	local glyph = glyph_in(app, scope, expand, arrows)
	expect(glyph.h * 16 / 24 + HEAVIEST_STROKE < FRAME_INK).to_be(true)
	expect(glyph.x + glyph.w / 2).to_be(arrows.x + arrows.w / 2)
	expect(glyph.y + glyph.h / 2).to_be(arrows.y + arrows.h / 2)
end

-- Eased all the way: Expand has stopped moving.
local function settled(app: any)
	local last = ""
	expect(eventually(app, function(): boolean
		app:frames(5)
		local found = box(app, "#shell", "panel")
		local now = app:tree("#titlebar") .. (if found then `{found.x}` else "")
		local still = now == last
		last = now
		return still
	end)).to_be(true)
	app:frames(30)
end

local function open_panel(): any
	local app = mount("shell_fixture.luau")
	app:resize(1200, 800)
	app:frames(30)
	expect(app:error()).to_be_nil()
	app:click('#shell-sidebar >> role:button "Fix the build"')
	app:frames(20)
	app:click('#titlebar >> role:button "Panel"')
	expect(app:wait("#panel-open-files", { timeout_ms = 15000 })).to_be(true)
	settled(app)
	return app
end

local function open_files(app: any)
	app:click("#panel-open-files")
	expect(app:wait("#panel-strip >> role:tab", { timeout_ms = 15000 })).to_be(true)
	settled(app)
end

test("Expand stands off the panel toggle as Back does off the sidebar toggle, level with them", function()
	local app = open_panel()
	expect_mirrored(app, "#shell", '"Expand panel"')
	open_files(app)
	expect_mirrored(app, "#shell", '"Expand panel"')
	expect(app:error()).to_be_nil()
end)

test("the corner holds with the panel expanded and with the sidebar collapsed", function()
	local app = open_panel()
	open_files(app)
	app:click("#panel-expand")
	settled(app)
	expect_mirrored(app, "#shell", '"Collapse panel"')
	app:click('#titlebar >> role:button "Toggle sidebar"')
	settled(app)
	expect_mirrored(app, "#shell", '"Collapse panel"')
	app:click("#panel-expand")
	settled(app)
	expect_mirrored(app, "#shell", '"Expand panel"')
	expect(app:error()).to_be_nil()
end)

