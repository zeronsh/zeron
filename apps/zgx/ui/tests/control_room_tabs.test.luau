-- The Control room as Board, Map, Chat and Notes (screens/control/page),
-- on ui/control_fleet_fixture.luau (a fleet as big as a person's, or a
-- small one): the Map at the page's size with two views, Needs you floating
-- on every tab but the Board, Q&A and the Scratchpad under Notes, no
-- widget dashboard, and an account with no orchestrator invited to make one.

local FLEET = "control_fleet_fixture.luau"

local function open(set: string, w: number?): any
	local app = mount(FLEET)
	app:resize(w or 1600, 1000)
	app:frames(20)
	app:command(set)
	app:frames(60)
	return app
end

local function tab(app: any, id: string)
	app:set_state("control_tab", id)
	app:frames(60)
end

-- An element's box: x, y, w, h.
local function box(app: any, target: string): (number, number, number, number)
	local line = string.match(app:tree(target, 0), "[^\n]+") or ""
	local x, y, w, h = string.match(line, " (%-?[%d%.]+),(%-?[%d%.]+) ([%d%.]+)x([%d%.]+)")
	return tonumber(x) or 0, tonumber(y) or 0, tonumber(w) or 0, tonumber(h) or 0
end

local function apart(app: any, a: string, b: string): boolean
	local ax, ay, aw, ah = box(app, a)
	local bx, by, bw, bh = box(app, b)
	return ax + aw <= bx or bx + bw <= ax or ay + ah <= by or by + bh <= ay
end

test("control room tabs: the page opens on the Board; no widget dashboard anywhere", function()
	local app = open("fleet_dense")
	expect(app:state("control_tab")).to_be("board")
	for _, t in { "board", "map", "chat", "notes" } do
		tab(app, t)
		expect(app:has("#control-columns")).to_be(false)
		expect(app:has(".control-widget")).to_be(false)
		expect(app:has("#control-widgets-menu")).to_be(false)
	end
	expect(app:error()).to_be_nil()
end)

test("control room tabs: the Map stands at the tab's top across its width, clear of Capture's bar", function()
	for _, w in { 1600, 1100 } do
		local app = open("fleet_dense", w)
		tab(app, "map")
		local _, ty, tw = box(app, "#control-map-tab")
		local _, my, mw = box(app, "#control-map")
		-- (Across the tab, less its sides; at its top. The Graph's own
		-- height is ds/orch_force's.)
		expect(mw > tw - 80).to_be(true)
		expect(my - ty < 24).to_be(true)
		expect(apart(app, "#control-map", "#control-float")).to_be(true)
	end
end)

test("control room tabs: the Graph takes the Map tab's height, past its own 760, inside its block", function()
	local fields = {}
	for _, h in { 1000, 1400 } do
		local app = mount(FLEET)
		app:resize(1600, h)
		app:frames(20)
		app:command("fleet_dense")
		tab(app, "map")
		app:frames(30)
		local _, my, _, mh = box(app, "#control-map")
		local _, fy, _, fh = box(app, "#orch-force-field")
		-- (Inside its block, and the block clear of Capture's bar.)
		expect(fy >= my and fy + fh <= my + mh).to_be(true)
		expect(apart(app, "#control-map", "#control-float")).to_be(true)
		-- (Most of the block: its head and padding the rest.)
		expect(mh - fh < 90).to_be(true)
		fields[h] = fh
	end
	-- Taller with the window, past the Graph's own cap.
	expect(fields[1400] - fields[1000] > 350).to_be(true)
	expect(fields[1400] > 760).to_be(true)
end)

test("control room tabs: the Map's menu offers the Graph and the Machines, nothing else", function()
	local app = open("fleet_dense")
	tab(app, "map")
	app:click("#control-map >> .control-panel-menu >> role:button")
	app:frames(20)
	expect(app:has('role:menuitemcheckbox "Graph"')).to_be(true)
	expect(app:has('role:menuitemcheckbox "Machines"')).to_be(true)
	for _, gone in { "Topology", "Timeline", "First graph", "Lines", "Tags" } do
		expect(app:has(`role:menu >> ~"{gone}"`)).to_be(false)
	end
	app:click('role:menuitemcheckbox "Machines"')
	app:frames(30)
	expect((app:state("control_map") :: any).variant).to_be("machines")
end)

test("control room tabs: a view kept from before that the Map no longer offers opens on the Graph", function()
	local app = mount(FLEET)
	app:set_state("control_map", { variant = "topology", window = 15 })
	app:resize(1600, 1000)
	app:frames(20)
	app:command("fleet_dense")
	app:set_state("control_tab", "map")
	app:frames(60)
	expect((app:state("control_map") :: any).variant).to_be("force")
end)

test("control room tabs: Needs you floats on the Map, Chat and Notes, not on the Board", function()
	local app = open("fleet_dense")
	expect(app:has("#control-needs-float")).to_be(false)
	for _, t in { "map", "chat", "notes" } do
		tab(app, t)
		expect(app:has("#control-needs-float")).to_be(true)
		expect(apart(app, "#control-needs-float", "#control-float")).to_be(true)
	end
	tab(app, "board")
	app:frames(20)
	expect(app:has("#control-needs-float")).to_be(false)
end)

test("control room tabs: Notes holds the Q&A and the Scratchpad, side by side when wide", function()
	local app = open("fleet_dense")
	tab(app, "notes")
	expect(app:has("#control-notes-qa >> #orch-qa")).to_be(true)
	expect(app:has("#control-notes-scratchpad >> #scratchpad")).to_be(true)
	local _, qy = box(app, "#control-notes-qa")
	local _, sy = box(app, "#control-notes-scratchpad")
	expect(qy).to_be(sy)
	local narrow = open("fleet_dense", 1000)
	tab(narrow, "notes")
	local _, ny = box(narrow, "#control-notes-qa")
	local _, my = box(narrow, "#control-notes-scratchpad")
	expect(ny < my).to_be(true)
end)

test("control room tabs: Capture's bar and its history on every tab", function()
	local app = open("fleet_sparse")
	for _, t in { "board", "map", "chat", "notes" } do
		tab(app, t)
		expect(app:has("#control-float >> #control-float-composer")).to_be(true)
		expect(app:has("#control-history-open")).to_be(true)
	end
	app:click("#control-history-open")
	app:frames(30)
	expect(app:has("#control-history >> #control-capture")).to_be(true)
	expect(apart(app, "#control-history", "#control-float")).to_be(true)
end)

test("control room tabs: an account with no orchestrator is invited to make one, no empty tabs", function()
	local app = mount(FLEET)
	app:resize(1600, 1000)
	app:frames(20)
	app:set_state("orchestration", { state = "ready", orchestrators = {}, crew = {}, items = {}, asks = {}, dumps = {} })
	app:frames(40)
	expect(app:has("#control-invite")).to_be(true)
	expect(app:has('#control-invite >> ~"orchestrator"')).to_be(true)
	-- (What it is, said whole: wrapped in its column, never cut.)
	local _, _, bw, bh = box(app, "#control-invite >> .control-invite-body")
	expect(bw <= 480).to_be(true)
	expect(bh > 30).to_be(true)
	local function hidden(target: string): boolean
		return string.find(string.match(app:tree(target, 0), "[^\n]+") or "", " hidden", 1, true) ~= nil
	end
	expect(hidden("#control-body")).to_be(true)
	expect(hidden("#control-tabs")).to_be(true)
	-- (Nothing to type a dump into yet: no bar.)
	expect(string.find(string.match(app:tree("#control-float-layer", 0), "[^\n]+") or "", " hidden", 1, true) ~= nil).to_be(true)
	app:click("#control-invite-create")
	app:frames(20)
	expect(app:state("orch_new_open")).to_be(true)
end)

test("control room tabs: the Brain dump alone still invites; a first orchestrator brings the tabs", function()
	local app = open("fleet_sparse")
	expect(app:has("#control-invite")).to_be(false)
	local n = table.clone(app:state("orchestration") :: any)
	local only = {}
	for _, o in n.orchestrators do
		if o.kind == "brain_dump" then
			table.insert(only, o)
		end
	end
	n.orchestrators = only
	app:set_state("orchestration", n)
	app:frames(40)
	expect(app:has("#control-invite")).to_be(true)
end)
