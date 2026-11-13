-- The Control room scrolled down stays where the person left it
-- (screens/control/page, ds/kept_scroll): as the world moves under it (a
-- report in Comms, a thread showing in Progress, a Board card deleted),
-- when its content is short for a moment and grows back, and when another
-- page opened from it is left for it again.

local function open(fixture: string?, h: number?): any
	local app = mount(fixture or "control_page_fixture.luau")
	app:resize(1440, h or 900)
	app:frames(30)
	return app
end

-- A person's pause: the view's moves after it aren't their scroll's
-- (ds/kept_scroll), as a test's frames come at once. (Frames drawn for at
-- least 0.6 s of the process's time, so as long of the clock's.)
local function pause(app: any)
	local from = os.clock()
	while os.clock() - from < 0.6 do
		app:frames(1)
	end
end

-- Where the page's head is drawn: its top, in the window.
local function top(app: any): number
	local line = string.match(app:tree("#control-widgets-menu"), "[^\n]+") or ""
	return tonumber(string.match(line, " %-?[%d%.]+,(%-?[%d%.]+) ")) or 0
end

-- Scrolled down `by`, then `change`: the head as far up after as before.
local function holds(fixture: string, by: number, change: (any) -> ())
	local app = open(fixture)
	local start = top(app)
	app:scroll("#control-scroll", by)
	app:frames(30)
	pause(app)
	local scrolled = top(app)
	expect(scrolled < start - 200).to_be(true)
	change(app)
	app:frames(30)
	expect(top(app)).to_be(scrolled)
	-- (The shell's fixture stands in for no `appearance` host action: its
	-- own error, not the page's.)
	if fixture == "control_page_fixture.luau" then
		expect(app:error()).to_be(nil)
	end
end

test("control scroll: a report in Comms keeps the place", function()
	holds("control_page_fixture.luau", 600, function(app)
		app:command("fixture.report")
	end)
end)

test("control scroll: a thread showing in Progress keeps the place", function()
	holds("control_page_fixture.luau", 600, function(app)
		app:command("fixture.progress_proxy")
		app:frames(5)
		app:command("fixture.activity")
	end)
end)

test("control scroll: a Board card deleted keeps the place", function()
	holds("control_fixture.luau", 2000, function(app)
		app:right_click('#control-widget-board >> role:button "Phase 2 migration"')
		app:frames(10)
		app:click('role:menuitem "Delete"')
	end)
end)

-- Every widget but Capture gone for a frame (as when they are built
-- again), then back: the page was too short to hold the place meanwhile.
test("control scroll: content short for a moment, the place comes back", function()
	holds("control_page_fixture.luau", 900, function(app)
		local all = app:state("control_widgets")
		app:set_state("control_widgets", {
			order = all.order,
			hidden = { needs = true, progress = true, map = true, board = true, machines = true, comms = true },
		})
		app:frames(2)
		app:set_state("control_widgets", all)
	end)
end)

-- Another page opened from it (a thread, Settings), and Back: the page
-- made again opens where it was.
test("control scroll: back from another page, the place is kept", function()
	holds("control_fixture.luau", 600, function(app)
		app:command("fixture.leave")
		app:frames(30)
		expect(app:has("#control-room")).to_be(false)
		app:command("fixture.control")
	end)
end)

-- Scrolled back to the top, it opens at the top.
test("control scroll: scrolled back up, the top is kept", function()
	local app = open("control_fixture.luau")
	local start = top(app)
	app:scroll("#control-scroll", 600)
	app:frames(30)
	app:scroll("#control-scroll", -2000)
	app:frames(30)
	expect(top(app)).to_be(start)
	app:command("fixture.leave")
	app:frames(30)
	app:command("fixture.control")
	app:frames(30)
	expect(top(app)).to_be(start)
end)

-- An orchestrator's page (screens/orchestrator/page, its live state's
-- scroll): kept as the page is made again while the host's orchestration
-- is a moment unknown.
test("orchestrator scroll: the live state keeps its place while orchestration is republished", function()
	local app = mount("orchestrator_fixture.luau")
	app:resize(1440, 700)
	app:frames(30)
	local function at(): number
		local line = string.match(app:tree("#orch-section-crew"), "[^\n]+") or ""
		return tonumber(string.match(line, " %-?[%d%.]+,(%-?[%d%.]+) ")) or 0
	end
	local start = at()
	app:scroll("#orch-live", 400)
	app:frames(30)
	pause(app)
	local scrolled = at()
	expect(scrolled < start - 100).to_be(true)
	app:command("orch.unknown")
	app:frames(5)
	expect(app:has("#orch-live")).to_be(false)
	app:command("orch.known")
	app:frames(30)
	expect(at()).to_be(scrolled)
end)
