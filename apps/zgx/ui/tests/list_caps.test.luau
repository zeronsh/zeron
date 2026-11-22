-- Every list the Control room and an orchestrator's page show that grows
-- with the work is capped, the rest a click away (ui/control_fleet_fixture:
-- a fleet as big as a person's, or a small one):
--
--   the Overview's Machines   working threads only, at most six a machine,
--                             "n more" opening the Map's Machines view
--   Needs you                 chats awaiting input with nothing to show are
--                             one line; "n more" opens the rest in a list
--                             that scrolls past about ten rows
-- and a small fleet shows what it did before.

local FLEET = "control_fleet_fixture.luau"

local function open(set: string, w: number?): any
	local app = mount(FLEET)
	app:resize(w or 1600, 1000)
	app:frames(20)
	app:command(set)
	app:frames(60)
	return app
end

-- An element's box: x, y, w, h.
local function box(app: any, target: string): (number, number, number, number)
	local line = string.match(app:tree(target, 0), "[^\n]+") or ""
	local x, y, w, h = string.match(line, " (%-?[%d%.]+),(%-?[%d%.]+) ([%d%.]+)x([%d%.]+)")
	return tonumber(x) or 0, tonumber(y) or 0, tonumber(w) or 0, tonumber(h) or 0
end

-- Every chat of the fleet, by id (crew threads and side chats too).
local function chats(app: any): { [string]: any }
	local s = app:state("sessions") :: any
	local out = {}
	for _, c in s.chats or {} do
		out[c.id] = c
	end
	for _, list in s.children or {} do
		for _, c in list do
			out[c.id] = c
		end
	end
	return out
end

-- The threads at work on a machine, as Machines counts them.
local function working_on(app: any, device: string): number
	local n = 0
	for _, c in chats(app) do
		if c.device == device and not c.archived and (c.status == "working" or c.status == "connecting") then
			n += 1
		end
	end
	return n
end

-- Machines -----------------------------------------------------------------

test("list caps: the Overview's Machines lists six working threads a machine, the rest on n more", function()
	local app = open("fleet_dense")
	app:set_state("control_scroll", 99999)
	app:frames(60)
	local machines = app:count(".orch-machine")
	expect(machines >= 4).to_be(true)
	for i = 1, machines do
		local m = `.orch-machine [{i}]`
		local jobs = app:count(`{m} >> .orch-machine-job`)
		expect(jobs <= 6).to_be(true)
	end
	-- This device's: far more than six at work, six shown, the rest counted.
	local here = working_on(app, "dev-desk")
	expect(here > 6).to_be(true)
	expect(app:has(`.orch-machine [1] >> .orch-machine-more >> ~"{here - 6} more"`)).to_be(true)
	-- No one waiting on the person listed: Needs you has those.
	expect(app:has(`.orch-machines >> ~"Own thread 9"`)).to_be(false)
	expect(app:error()).to_be_nil()
end)

test("list caps: a machine's n more opens the Map's Machines view, in view", function()
	local app = open("fleet_dense")
	-- (Its first machine's "n more" brought into view.)
	local _, top = box(app, "#control-scroll")
	local _, y = box(app, ".orch-machine [1] >> .orch-machine-more")
	app:set_state("control_scroll", math.max(0, y - top - 300))
	app:frames(60)
	app:click(".orch-machine [1] >> .orch-machine-more")
	app:frames(60)
	local view = app:state("control_map") :: any
	expect(view.variant).to_be("machines")
	-- (The Map brought up to the view's top.)
	local _, my = box(app, "#control-widget-map")
	expect(math.abs(my - top) < 80).to_be(true)
end)

test("list caps: the dense Overview is a few screens, not ten", function()
	local app = open("fleet_dense")
	app:frames(60)
	local _, _, _, h = box(app, "#control-columns")
	local _, _, _, machines = box(app, "#control-widget-machines")
	-- (It was about 9,400 points, Machines most of it.)
	print("dense Overview columns", h, "Machines", machines)
	expect(h < 4000).to_be(true)
	expect(machines < 1800).to_be(true)
end)

test("list caps: a small fleet's Machines lists every working thread, no n more", function()
	local app = open("fleet_sparse")
	app:set_state("control_scroll", 99999)
	app:frames(60)
	expect(app:count(".orch-machine-job")).to_be(working_on(app, "dev-desk"))
	expect(app:has(".orch-machine-more")).to_be(false)
end)

-- Needs you ----------------------------------------------------------------

local NEEDS = "#control-widget-needs"

-- The chats awaiting input with no ask of theirs: what the line counts.
local function bare_inputs(app: any): number
	local asking = {}
	for _, a in (app:state("orchestration") :: any).asks or {} do
		if a.state == "open" then
			asking[a.from_chat] = true
		end
	end
	local n = 0
	for id, c in chats(app) do
		if (c.status == "input" or c.status == "approval") and not c.archived and not asking[id] then
			n += 1
		end
	end
	return n
end

test("list caps: Needs you folds chats awaiting input into one line, which opens", function()
	local app = open("fleet_dense")
	local n = bare_inputs(app)
	expect(n > 1).to_be(true)
	app:click(`{NEEDS} >> .orch-asks-more`)
	app:frames(30)
	expect(app:count(`{NEEDS} >> .orch-need-inputs`)).to_be(1)
	expect(app:has(`{NEEDS} >> .orch-need-inputs >> ~"{n} chats waiting for input"`)).to_be(true)
	-- (Folded: none of their rows.)
	expect(app:has(`{NEEDS} >> .orch-need-inputs >> .orch-need`)).to_be(false)
	app:click(`{NEEDS} >> .orch-need-inputs-toggle`)
	app:frames(30)
	expect(app:count(`{NEEDS} >> .orch-need-inputs >> .orch-need`)).to_be(n)
end)

test("list caps: Needs you's n more opens the rest in a list that scrolls past ten rows", function()
	local app = open("fleet_dense")
	-- (Six rows: the chats awaiting input are one of them.)
	local before = app:count(`{NEEDS} >> .orch-need`) + app:count(`{NEEDS} >> .orch-need-inputs`)
	expect(before).to_be(6)
	expect(app:has(`{NEEDS} >> .orch-asks-more >> ~"more"`)).to_be(true)
	local _, _, _, closed = box(app, NEEDS)
	app:click(`{NEEDS} >> .orch-asks-more`)
	app:frames(40)
	local _, _, _, list_h = box(app, `{NEEDS} >> .orch-asks-list [1]`)
	local _, _, _, opened = box(app, NEEDS)
	-- Ten rows or so, not every one: the list scrolls.
	local rows = app:count(`{NEEDS} >> .orch-asks-list >> .orch-need`) + app:count(`{NEEDS} >> .orch-need-inputs`)
	expect(rows > 10).to_be(true)
	expect(list_h < 10 * 140).to_be(true)
	expect(opened - closed < 10 * 140).to_be(true)
	expect(app:error()).to_be_nil()
end)

test("list caps: an orchestrator's Needs you caps the same way", function()
	local app = open("fleet_dense")
	app:command("fleet_orch_orc-ui")
	app:frames(90)
	local S = "#orch-section-needs"
	expect(app:count(`{S} >> .orch-need`) <= 6).to_be(true)
	app:click(`{S} >> .orch-asks-more`)
	app:frames(40)
	local _, _, _, list_h = box(app, `{S} >> .orch-asks-list [1]`)
	expect(list_h < 10 * 140).to_be(true)
end)

test("list caps: a small fleet's Needs you is as it was", function()
	local app = open("fleet_sparse")
	expect(app:count(`{NEEDS} >> .orch-need`)).to_be(1)
	expect(app:has(`{NEEDS} >> .orch-need-inputs`)).to_be(false)
	expect(app:has(`{NEEDS} >> .orch-asks-more`)).to_be(false)
end)

test("list caps: Progress's n more opens the rest in a list that scrolls", function()
	local app = open("fleet_dense")
	app:frames(60)
	expect(app:has("#control-progress-more")).to_be(true)
	local _, top = box(app, "#control-scroll")
	local _, y = box(app, "#control-widget-progress")
	app:set_state("control_scroll", math.max(0, y - top - 20))
	app:frames(40)
	local _, _, _, closed = box(app, "#control-widget-progress")
	app:click("#control-progress-more")
	app:frames(40)
	local _, _, _, opened = box(app, "#control-widget-progress")
	local _, _, _, list_h = box(app, ".control-progress-list [1]")
	expect(list_h <= 800).to_be(true)
	expect(opened - closed <= 800).to_be(true)
end)

test("list caps: the chats awaiting input, opened, scroll past ten", function()
	local app = open("fleet_dense")
	local n = bare_inputs(app)
	expect(n > 10).to_be(true)
	app:click(`{NEEDS} >> .orch-asks-more`)
	app:frames(30)
	app:click(`{NEEDS} >> .orch-need-inputs-toggle`)
	app:frames(30)
	local _, _, _, h = box(app, `{NEEDS} >> .orch-need-inputs-list [1]`)
	expect(h <= 10 * 52).to_be(true)
end)
