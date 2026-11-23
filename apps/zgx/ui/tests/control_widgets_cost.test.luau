-- What Capture and Needs you build and hold (screens/control/page,
-- ds/orch_asks): the history builds its latest few rows, more on a click;
-- a new dump or ask builds its own row and leaves the others' nodes as
-- they were; Capture's composer takes in the Brain dump's chat (its
-- transcript, hundreds of routing turns) only once it's used. On
-- ui/control_page_fixture.luau: Capture's history opened from its bar,
-- Needs you floating over the Map.

local function open(): any
	local app = mount("control_page_fixture.luau")
	app:resize(1440, 900)
	app:frames(30)
	app:set_state("control_history_panel", true)
	app:frames(30)
	return app
end

-- A node's identity, by its kind and label (or text): `@id`.
local function node(app: any, kind: string, label: string, within: string?): string?
	local pattern = ` {kind} "{label}"`
	for line in app:tree(within or "#control-capture"):gmatch("[^\n]+") do
		if line:find(pattern, 1, true) then
			return line:match("@(%d+)")
		end
	end
	return nil
end

-- The engine's state with `n` dumps (the sample's first, the rest earlier
-- ones made from it, newest first).
local function with_dumps(app: any, n: number)
	local now = table.clone(app:state("orchestration"))
	local base = now.dumps[1]
	local dumps = {}
	for i = 1, n do
		local d = table.clone(base)
		if i > 1 then
			d.id = `dp-old-{i}`
			d.text = `earlier dump number {i}`
			d.created = (base.created or 0) - i * 60
		end
		table.insert(dumps, d)
	end
	now.dumps = dumps
	app:set_state("orchestration", now)
	app:frames(30)
end

local NEEDS = "#control-needs-float"

-- The history scrolled to bring its "more" into view.
local function to_foot(app: any)
	app:scroll("#control-history-scroll", 4000)
	app:frames(10)
end

local function open_history(app: any)
	app:click("#control-history-toggle")
	app:frames(30)
end

test("the history builds its latest ten rows, ten more on each click", function()
	local app = open()
	with_dumps(app, 26)
	open_history(app)
	expect(app:error()).to_be_nil()
	expect(app:count(".control-history-row")).to_be(10)
	expect(app:has('role:button "earlier dump number 11"')).to_be(true)
	expect(app:has('role:button "earlier dump number 12"')).to_be(false)
	expect(app:text("#control-history-more")).to_be("15 more")
	to_foot(app)
	app:click("#control-history-more")
	app:frames(30)
	expect(app:count(".control-history-row")).to_be(20)
	to_foot(app)
	app:click("#control-history-more")
	app:frames(30)
	expect(app:count(".control-history-row")).to_be(25)
	expect(app:has("#control-history-more")).to_be(false)
	expect(app:error()).to_be_nil()
end)

test("a new dump builds one history row; the rows already there keep their nodes", function()
	local app = open()
	with_dumps(app, 8)
	open_history(app)
	local before: { [string]: string? } = {}
	for i = 2, 8 do
		before[i] = node(app, "button", `earlier dump number {i}`)
		expect(before[i]).never.to_be_nil()
	end
	expect(app:count(".control-history-row")).to_be(7)
	app:type("#control-float >> role:textbox", "a brand new dump")
	app:key("Enter")
	app:frames(30)
	expect(app:error()).to_be_nil()
	-- (The sample's newest moved into the history: one row more.)
	expect(app:count(".control-history-row")).to_be(8)
	for i = 2, 8 do
		expect(`row {i} @{node(app, "button", `earlier dump number {i}`)}`).to_be(`row {i} @{before[i]}`)
	end
end)

test("a dump past the rows shown, asked for (a card's source), widens the history to it", function()
	local app = open()
	with_dumps(app, 30)
	expect(app:has('role:button "earlier dump number 23"')).to_be(false)
	app:set_state("orchestration_reveal_dump", "dp-old-23")
	app:frames(60)
	expect(app:error()).to_be_nil()
	expect(app:has('role:button "earlier dump number 23"')).to_be(true)
	-- (Ten at a time: as many tens as reach it.)
	expect(app:count(".control-history-row")).to_be(29)
end)

test("a new ask builds its own row; the rows already there keep their nodes", function()
	local app = open()
	app:set_state("control_history_panel", false)
	app:set_state("control_tab", "map")
	app:frames(30)
	if app:has(`{NEEDS} >> .orch-asks-more`) then
		app:click(`{NEEDS} >> .orch-asks-more`)
		app:frames(20)
	end
	local count = app:count(`{NEEDS} >> .orch-need`)
	-- (The board's three Needs-you cards.)
	expect(count).to_be(3)
	local approve = node(app, "button", "Approve", NEEDS)
	local side = node(app, "button", "Side by side", NEEDS)
	expect(approve).never.to_be_nil()
	app:command("fixture.turn_end_ask")
	app:frames(30)
	expect(app:error()).to_be_nil()
	expect(app:count(`{NEEDS} >> .orch-need`)).to_be(count + 1)
	expect(`approve @{node(app, "button", "Approve", NEEDS)}`).to_be(`approve @{approve}`)
	expect(`side @{node(app, "button", "Side by side", NEEDS)}`).to_be(`side @{side}`)
	-- The engine's state again, every ask a fresh copy: no row made again.
	local turn = node(app, "button", "Merge into zeron/orchestrators", NEEDS)
	expect(turn).never.to_be_nil()
	app:command("fixture.republish")
	app:frames(10)
	expect(`approve @{node(app, "button", "Approve", NEEDS)}`).to_be(`approve @{approve}`)
	expect(`turn @{node(app, "button", "Merge into zeron/orchestrators", NEEDS)}`).to_be(`turn @{turn}`)
end)

test("Capture's composer takes in the Brain dump's chat only once it's used", function()
	local app = open()
	-- Shown, not used: nothing watched.
	expect((app:state("watched") or {})["control-capture"]).to_be_nil()
	app:hover("#control-float >> role:textbox")
	app:frames(5)
	expect((app:state("watched") or {})["control-capture"]).never.to_be_nil()
	expect(app:error()).to_be_nil()
end)
