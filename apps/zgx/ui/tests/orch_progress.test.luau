-- How a thread's work is going (ds/orch_progress, ui/orch_progress_fixture):
-- the clocks tick from the row's times on the fixture's clock, the steps
-- and the time left read as the row says (none without an ETA), stalled
-- in the warning's words, a fresh note on its way shown, "What's up?"
-- asked once then resting, each variant drawn and resized.

local function unit(): any
	return require("@ds/orch_progress_clocks")
end

local function open(w: number?, h: number?): any
	local app = mount("orch_progress_fixture.luau")
	app:resize(w or 1240, h or 1000)
	app:frames(10)
	return app
end

local ENGINE = "#fx-full-th-engine-state"

-- What a rolling number shows: its cells' figures at rest on its line.
local function face(app: any, target: string): string
	local tree = app:tree(target)
	local top = tree:match(" %-?%d+,(%-?%d+) ")
	local out = {}
	for line in tree:gmatch("[^\n]+") do
		local t, y = line:match('text "(.-)" %-?%d+,(%-?%d+) ')
		if t and y == top and not line:find(" hidden") then
			table.insert(out, t)
		end
	end
	return table.concat(out)
end
local NOW = 1794470400

test("orch progress: the clocks, as the contract has them", function()
	local Unit = unit()
	local r = { state = "working", started = NOW - 600, last_step = NOW - 90, tool_since = NOW - 540, eta_secs = 300, updated = NOW - 60 }
	local c = Unit.clocks(r, NOW)
	expect(c.elapsed).to_be(600)
	expect(c.since_step).to_be(90)
	expect(c.tool).to_be(540)
	-- Left: the ETA less what passed since it was written.
	expect(c.left).to_be(240)
	-- None left: none said.
	expect(Unit.clocks(r, NOW + 400).left).to_be(nil)
	-- No ETA: no time left, never a guess.
	local plain = { state = "working", started = NOW - 600, updated = NOW }
	expect(Unit.clocks(plain, NOW).left).to_be(nil)
	expect(Unit.clocks(plain, NOW).since_step).to_be(nil)
	-- Over: its clock stops where it ended; an ETA isn't read.
	local done = { state = "done", started = NOW - 3600, eta_secs = 300, tool_since = NOW - 10, updated = NOW - 600 }
	expect(Unit.clocks(done, NOW).elapsed).to_be(3000)
	expect(Unit.clocks(done, NOW).left).to_be(nil)
	expect(Unit.clocks(done, NOW).tool).to_be(nil)
	-- Waiting on you or on background work: still on it, still running.
	expect(Unit.clocks({ state = "awaiting_input", started = NOW - 60, updated = NOW - 50 }, NOW).elapsed).to_be(60)
	expect(Unit.clock(42)).to_be("0:42")
	expect(Unit.clock(21 * 60 + 3)).to_be("21:03")
	expect(Unit.clock(3600 + 4 * 60 + 12)).to_be("1:04:12")
end)

test("orch progress: what it is on, the live label while a command runs", function()
	local Unit = unit()
	local r = { state = "working", step = "Ops", tool = "cargo test", tool_since = NOW - 540 }
	expect(Unit.doing(r, { activity = { kind = "testing", label = "cargo test -p zeron-engine", since = NOW - 5 } })).to_be("cargo test -p zeron-engine")
	expect(Unit.doing(r, nil)).to_be("cargo test")
	expect(Unit.step(r)).to_be("Ops")
	expect(Unit.step({ state = "working", step = "cargo test", tool = "cargo test" })).to_be(nil)
	expect(Unit.doing({ state = "awaiting_input", tool = "x" }, nil)).to_be(nil)
	expect(Unit.kind({ state = "awaiting_input" })).to_be("input")
	expect(Unit.kind({ state = "waiting" })).to_be("waiting")
	expect(Unit.kind({ state = "idle" })).to_be("idle")
	expect(Unit.stalled({ state = "working", stalled = true })).to_be(true)
	expect(Unit.stalled({ state = "done", stalled = true })).to_be(false)
end)

test("orch progress: a full unit's figures, and its clocks ticking", function()
	local app = open()
	-- Elapsed, steps, time left, the command and how long it has run, the
	-- last step, the note.
	expect(face(app, `{ENGINE}-elapsed`)).to_be("41:00")
	expect(app:has(`{ENGINE} >> .orch-progress-steps >> ~"5/8"`)).to_be(true)
	expect(app:has(`{ENGINE} >> ~"~20m left"`)).to_be(true)
	expect(app:has(`{ENGINE} >> .orch-progress-doing >> ~"cargo test -p zeron-engine --lib orchestration"`)).to_be(true)
	expect(app:has(`{ENGINE} >> ~"· 9m"`)).to_be(true)
	expect(app:has(`{ENGINE} >> ~"last step 2m ago"`)).to_be(true)
	expect(app:has(`{ENGINE} >> ~"Registry kinds and the watch are in"`)).to_be(true)
	expect(app:has(`{ENGINE} >> ~"Orchestrators · desk"`)).to_be(true)
	-- A minute and five seconds on.
	app:set_state("now", NOW + 65)
	app:frames(60)
	expect(face(app, `{ENGINE}-elapsed`)).to_be("42:05")
	expect(app:has(`{ENGINE} >> ~"last step 3m ago"`)).to_be(true)
	expect(app:has(`{ENGINE} >> ~"~19m left"`)).to_be(true)
	expect(app:has(`{ENGINE} >> ~"· 10m"`)).to_be(true)
	expect(app:error()).to_be(nil)
end)

test("orch progress: the elapsed clock keeps its width as it ticks", function()
	local app = open()
	local function box(): string?
		local line = app:tree(`#fx-full-th-engine-state-elapsed`):match("[^\n]+")
		return line and line:match("(%d+x%d+)")
	end
	local before = box()
	expect(before).never.to_be(nil)
	for i = 1, 12 do
		app:set_state("now", NOW + i * 7)
		app:frames(4)
		expect(box()).to_be(before)
	end
end)

test("orch progress: without an ETA, no time left", function()
	local app = open()
	-- Waiting on you: its steps, never a time left.
	expect(app:has(`#fx-full-th-ui-orch >> ~"4/6"`)).to_be(true)
	expect(app:has(`#fx-full-th-ui-orch >> .orch-progress-left`)).to_be(false)
	expect(app:has(`#fx-full-th-ui-orch >> ~"Awaiting"`)).to_be(true)
	-- Its ETA gone from the row: gone from the unit.
	local rows = table.clone(app:state("rows") :: any)
	local r = table.clone(rows["th-engine-state"])
	r.eta_secs = nil
	rows["th-engine-state"] = r
	app:set_state("rows", rows)
	app:frames(5)
	expect(app:has(`{ENGINE} >> .orch-progress-left`)).to_be(false)
	expect(app:has(`#fx-strip-th-engine-state >> .orch-progress-left`)).to_be(false)
end)

test("orch progress: stalled, in the warning's words", function()
	local app = open()
	local UNIT = "#fx-full-th-ui-control"
	expect(app:has(`{UNIT} >> .orch-progress-stalled >> ~"Stalled"`)).to_be(true)
	expect(app:has(`{UNIT} >> .orch-progress-stalled-glyph`)).to_be(true)
	expect(app:has(`{UNIT} >> ~"last step 19m ago"`)).to_be(true)
	-- (Stalled says it in place of its steps' figure.)
	expect(app:has(`{UNIT} >> .orch-progress-head >> .orch-progress-steps`)).to_be(false)
	expect(app:has(`#fx-strip-th-ui-control >> .orch-progress-stalled-glyph`)).to_be(true)
	-- Going again: the warning goes (after its 300 ms).
	local rows = table.clone(app:state("rows") :: any)
	local r = table.clone(rows["th-ui-control"])
	r.stalled = false
	r.last_step = NOW - 5
	rows["th-ui-control"] = r
	app:set_state("rows", rows)
	expect(app:wait(`{UNIT} >> .orch-progress-stalled`, { gone = true })).to_be(true)
	expect(app:has(`{UNIT} >> .orch-progress-steps >> ~"2/7"`)).to_be(true)
end)

test("orch progress: a fresh note on its way shows its roller", function()
	local app = open()
	expect(app:has("#fx-full-th-engine-flow >> .orch-progress-refreshing")).to_be(true)
	expect(app:has("#fx-full-th-engine-state >> .orch-progress-refreshing")).to_be(false)
	local rows = table.clone(app:state("rows") :: any)
	local r = table.clone(rows["th-engine-flow"])
	r.refreshing = false
	r.note = "Report tool wired; tests next."
	rows["th-engine-flow"] = r
	app:set_state("rows", rows)
	app:frames(5)
	expect(app:has("#fx-full-th-engine-flow >> .orch-progress-refreshing")).to_be(false)
	expect(app:has(`#fx-full-th-engine-flow >> ~"Report tool wired; tests next."`)).to_be(true)
end)

test("orch progress: What's up? asks once, then rests 15 s", function()
	local app = open()
	app:click(`{ENGINE} >> .orch-progress-check`)
	app:frames(3)
	expect(app:state("calls")).to_be("check:th-engine-state")
	-- Resting: asked again, nothing.
	app:click(`{ENGINE} >> .orch-progress-check`)
	app:frames(3)
	expect(app:state("calls")).to_be("check:th-engine-state")
	-- 15 s on (frames are the test's time), it can be asked again.
	app:frames(15 * 60 + 5)
	app:click(`{ENGINE} >> .orch-progress-check`)
	app:frames(3)
	expect(app:state("calls")).to_be("check:th-engine-state,check:th-engine-state")
end)

test("orch progress: pinned and opened from its menu, the pin shown", function()
	local app = open()
	app:click(`{ENGINE} >> .orch-progress-menu`)
	app:frames(10)
	app:click('role:menuitem "Pin to Progress"')
	app:frames(10)
	expect(app:state("pins")["th-engine-state"]).to_be(true)
	expect(app:has(`{ENGINE} >> .orch-progress-pinned`)).to_be(true)
	-- A strip's menu: What's up?, open, pin.
	app:click(`#fx-strip-th-vpn-merge >> .orch-progress-menu`)
	app:frames(10)
	expect(app:has('role:menuitem "What\'s up?"')).to_be(true)
	app:click('role:menuitem "Pin to Progress"')
	app:frames(10)
	expect(app:state("pins")["th-vpn-merge"]).to_be(true)
	app:click(`{ENGINE} >> .orch-progress-menu`)
	app:frames(10)
	expect(app:has('role:menuitem "Unpin from Progress"')).to_be(true)
	app:click('role:menuitem "Open"')
	app:frames(10)
	expect(app:state("calls")).to_be("pin:th-engine-state:on,pin:th-vpn-merge:on,open:th-engine-state")
end)

test("orch progress: strips and chips say each state", function()
	local app = open()
	-- Working: its cells, steps, the command, the clocks.
	expect(app:has(`#fx-strip-th-engine-state >> .orch-progress-cells`)).to_be(true)
	expect(app:has(`#fx-strip-th-engine-state >> ~"5/8"`)).to_be(true)
	expect(app:has(`#fx-strip-th-engine-state >> ~"cargo test -p zeron-engine"`)).to_be(true)
	-- Waiting on background work, done, failed: their names.
	expect(app:has(`#fx-strip-th-vpn-merge >> ~"6/7"`)).to_be(true)
	expect(app:has(`#fx-strip-th-vpn-moscow >> ~"3/3"`)).to_be(true)
	expect(app:has(`#fx-strip-th-ci-cache >> .orch-progress-doing`)).to_be(true)
	-- An orchestrator: no todo list, no cells.
	expect(app:has(`#fx-strip-orc-orch >> .orch-progress-cells`)).to_be(false)
	-- Lean: no status, cells or activity; its clocks and menu.
	expect(app:has(`#fx-strip-lean-th-engine-state >> .orch-progress-cells`)).to_be(false)
	expect(app:has(`#fx-strip-lean-th-engine-state >> .orch-progress-doing`)).to_be(false)
	expect(app:has(`#fx-strip-lean-th-engine-state >> ~"last step 2m ago"`)).to_be(true)
	expect(app:has(`#fx-strip-lean-th-engine-state >> .orch-progress-menu`)).to_be(true)
	-- Chips: the time and steps.
	expect(app:has(`#fx-chip-th-engine-state >> ~"41m"`)).to_be(true)
	expect(app:has(`#fx-chip-th-engine-state >> ~"5/8"`)).to_be(true)
end)

test("orch progress: a row gone, its unit says nothing; drawn and resized", function()
	local app = open()
	for _, size in { { 1240, 1000 }, { 900, 1200 }, { 420, 1600 }, { 1440, 900 } } do
		app:resize(size[1], size[2])
		app:frames(10)
		expect(app:has(ENGINE)).to_be(true)
		expect(app:error()).to_be(nil)
	end
	app:set_state("rows", {})
	app:frames(10)
	expect(app:has(`{ENGINE} >> .orch-progress-steps`)).to_be(false)
	expect(app:has(`{ENGINE} >> .orch-counter`)).to_be(false)
	expect(app:error()).to_be(nil)
end)
