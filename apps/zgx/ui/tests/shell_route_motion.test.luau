-- The window's layout across a route change (ui/shell_fixture.luau, the
-- shell as it is): another session, or a new chat, lands at once (its panel
-- shown at its width or gone on the first frame, no step); the person's own
-- toggle of the panel still eases.

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

local function panel_width(app: any): number
	for line in app:tree("#shell"):gmatch("[^\n]+") do
		if line:find(" #shell-files", 1, true) then
			return tonumber(line:match(" %-?%d+,%-?%d+ (%d+)x%d+")) or 0
		end
	end
	return 0
end

local function open(): any
	local app = mount("shell_fixture.luau")
	app:resize(1200, 800)
	app:frames(30)
	expect(app:error()).to_be_nil()
	return app
end

-- (Settled: the width stays put across a few frames.)
local function settled(app: any): number
	local last = -1
	expect(eventually(app, function(): boolean
		app:frames(5)
		local w = panel_width(app)
		local still = w == last
		last = w
		return still
	end)).to_be(true)
	return last
end

local function show_chat(app: any, title: string)
	app:click(`#shell-sidebar >> role:button "{title}"`)
end

test("switching to a chat with its panel open or closed lands at once, no step", function()
	local app = open()
	show_chat(app, "Fix the build")
	app:frames(20)
	app:click('#titlebar >> role:button "Panel"')
	local open_w = settled(app)
	expect(open_w > 300).to_be(true)
	-- To a chat whose panel is closed: gone on the first frame.
	show_chat(app, "Write the docs")
	app:frames(1)
	expect(panel_width(app)).to_be(0)
	-- And back: at its width on the first frame.
	show_chat(app, "Fix the build")
	app:frames(1)
	expect(panel_width(app)).to_be(open_w)
	-- A new chat has no panel: gone at once; back to the chat, there at once.
	app:click('#shell-sidebar >> role:button "New chat"')
	app:frames(1)
	expect(panel_width(app)).to_be(0)
	show_chat(app, "Fix the build")
	app:frames(1)
	expect(panel_width(app)).to_be(open_w)
end)

test("the person's own toggle of the panel still eases open and shut", function()
	local app = open()
	show_chat(app, "Fix the build")
	app:frames(20)
	app:click('#titlebar >> role:button "Panel"')
	app:frames(3)
	local mid = panel_width(app)
	local open_w = settled(app)
	expect(mid > 0 and mid < open_w).to_be(true)
	app:click('#titlebar >> role:button "Panel"')
	app:frames(3)
	local closing = panel_width(app)
	expect(closing > 0 and closing < open_w).to_be(true)
end)

-- (The composer between a new chat and a session, its fade and its place,
-- is read in src/route_motion_tests.rs: these tests can't read an opacity.)

-- (The person's own sequence, Nov 2: a width chosen narrower than the
-- panel's least without its tree, then from a chat with no panel to one
-- with it. Its width while hidden was pushed to that least, so it arrived
-- wide and eased narrower.)
test("arriving at a chat whose panel is open at a chosen width, it's that width from the first frame", function()
	local app = open()
	app:resize(1283, 800)
	app:frames(30)
	show_chat(app, "Fix the build")
	app:frames(20)
	app:click('#titlebar >> role:button "Panel"')
	settled(app)
	-- Keyed narrower: a width chosen.
	app:click('#shell-files >> role:slider')
	for _ = 1, 12 do
		app:key("ArrowRight")
	end
	local chosen = settled(app)
	for _ = 1, 3 do
		show_chat(app, "Write the docs")
		expect(settled(app)).to_be(0)
		show_chat(app, "Fix the build")
		local seen = {}
		local wrong = false
		for _ = 1, 20 do
			app:frames(1)
			local w = panel_width(app)
			table.insert(seen, tostring(w))
			wrong = wrong or w ~= chosen
		end
		if wrong then
			error(`arrived at {table.concat(seen, " ")}, chosen {chosen}`)
		end
	end
end)

-- (And the window resized meanwhile, on the chat with no panel: what the
-- panel stands at there is worked out as it arrives, at once.)
test("arriving after the window resized on a chat with no panel, the panel stands at its width from the first frame", function()
	local app = open()
	app:resize(1400, 800)
	app:frames(30)
	show_chat(app, "Fix the build")
	app:frames(20)
	app:click('#titlebar >> role:button "Panel"')
	settled(app)
	app:click('#shell-files >> role:slider')
	for _ = 1, 4 do
		app:key("ArrowLeft")
	end
	settled(app)
	for _, w in { 1150, 1000, 1300 } do
		show_chat(app, "Write the docs")
		expect(settled(app)).to_be(0)
		app:resize(w, 800)
		app:frames(10)
		show_chat(app, "Fix the build")
		local seen = {}
		for _ = 1, 20 do
			app:frames(1)
			table.insert(seen, panel_width(app))
		end
		local final = settled(app)
		for i, v in seen do
			if v ~= final then
				error(`at {w}: frame {i} {v}, settling at {final} ({table.concat(seen, " ")})`)
			end
		end
	end
end)
