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
	app:click('#shell-chats >> role:slider "Resize files"')
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
	app:click('#shell-chats >> role:slider "Resize files"')
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

-- The panel's own motion (ds/motion `panel`, `content`), as Zeron's
-- `panel_content_tween`: opening, what's in it is laid out once, at the
-- width it's going to (closing, the one it showed at, expanded or not),
-- and stands at the window's right as its edge sweeps over it; between two
-- open widths (expanding, collapsing) it follows its edge a frame at a
-- time, never jumping to the far width at once. Under the panel expanding
-- over it, the page keeps its width, clipped, and takes its own back in
-- one go, rather than squeezing to nothing and growing from it. (Each
-- stands still and only fades a little as it comes and goes:
-- src/route_motion_tests.rs reads that.)
-- (Its width while it's laid out: shut, the panel leaves layout.)
local function width_of(app: any, id: string): number?
	for line in app:tree("#shell"):gmatch("[^\n]+") do
		if line:find(` #{id} `, 1, true) and not line:find(" hidden", 1, true) then
			return tonumber(line:match(" %-?%d+,%-?%d+ (%d+)x%d+"))
		end
	end
	return nil
end
test("the panel's content keeps one width as the panel opens and closes, follows it as it expands and collapses, and the page under it keeps one", function()
	local app = open()
	show_chat(app, "Fix the build")
	app:frames(20)
	-- Each width `id` takes over a step's frames, in order, once each.
	local function widths(click: string, ids: { string }): { [string]: { number } }
		app:click(click)
		local seen: { [string]: { number } } = {}
		for _ = 1, 30 do
			app:frames(1)
			for _, id in ids do
				local w = width_of(app, id)
				local list = seen[id] or {}
				seen[id] = list
				if w and list[#list] ~= w then
					table.insert(list, w)
				end
			end
		end
		return seen
	end
	local function one(seen: { [string]: { number } }, id: string, step: string): number
		local list = seen[id] or {}
		expect(`{step}: {id} at {table.concat(list, ", ")}`).to_be(`{step}: {id} at {list[1]}`)
		return list[1]
	end
	local CONTENT, PAGE = "shell-files-content", "shell-page-room"
	local opening = widths('#titlebar >> role:button "Panel"', { CONTENT, PAGE })
	local rest = one(opening, CONTENT, "opening")
	-- (The page beside it follows its edge, as upstream chose: #1207.)
	expect(#opening[PAGE] > 4).to_be(true)
	local beside = opening[PAGE][#opening[PAGE]]
	-- From `from` to `to` a frame at a time, one way, never more than half
	-- the way in a frame.
	local function follows(seen: { [string]: { number } }, step: string, from: number, to: number)
		local list = seen[CONTENT] or {}
		local all = `{step}: {table.concat(list, ", ")}`
		expect(#list >= 4).to_be(true)
		expect(list[#list]).to_be(to)
		local was = from
		for _, w in list do
			expect(math.abs(w - was) <= math.abs(to - from) / 2 and (w - was) * (to - from) >= 0 and all or "").to_be(all)
			was = w
		end
	end
	local expanding = widths("#panel-expand", { CONTENT, PAGE })
	local wide = (expanding[CONTENT] or {})[#(expanding[CONTENT] or {})] or 0
	expect(wide > rest + 200).to_be(true)
	follows(expanding, "expanding", rest, wide)
	expect(one(expanding, PAGE, "expanding")).to_be(beside)
	local collapsing = widths("#panel-expand", { CONTENT, PAGE })
	follows(collapsing, "collapsing", wide, rest)
	expect(one(collapsing, PAGE, "collapsing")).to_be(beside)
	local closing = widths('#titlebar >> role:button "Panel"', { CONTENT })
	expect(one(closing, CONTENT, "closing")).to_be(rest)
	expect(panel_width(app)).to_be(0)
	expect(width_of(app, CONTENT)).to_be_nil()
	-- Closed from expanded: at the wide width it showed.
	widths('#titlebar >> role:button "Panel"', { CONTENT })
	widths("#panel-expand", { CONTENT })
	expect(one(widths('#titlebar >> role:button "Panel"', { CONTENT }), CONTENT, "closing expanded")).to_be(wide)
end)
