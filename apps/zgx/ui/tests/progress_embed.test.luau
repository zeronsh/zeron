-- Progress where a thread is shown (screens/control/progress's strip): a
-- crew row's line and a board card's on an orchestrator's page
-- (ui/orchestrator_fixture.luau) and in the Control room
-- (ui/control_page_fixture.luau), pinned from its menu; and a working
-- session's line in its head (ui/control_fixture.luau, the window).

test("progress embed: crew rows and board cards on an orchestrator's page", function()
	local app = mount("orchestrator_fixture.luau")
	app:resize(1440, 1400)
	app:frames(40)
	local CREW = "#orch-section-crew"
	-- Each crew thread with a row: its clocks and menu, lean (the row
	-- already says its state, steps and activity).
	expect(app:has(`{CREW} >> #progress-strip-th-engine-state`)).to_be(true)
	expect(app:count(`{CREW} >> .orch-progress-lean`)).to_be(4)
	expect(app:has(`{CREW} >> #progress-strip-th-engine-state >> ~"last step 2m ago"`)).to_be(true)
	expect(app:has(`{CREW} >> #progress-strip-th-ui-control >> .orch-progress-menu`)).to_be(true)
	-- Its board's cards with a thread under way.
	expect(app:has(`#orch-page-board >> #progress-strip-th-engine-flow`)).to_be(true)
	expect(app:error()).to_be(nil)
	for _, size in { { 900, 1400 }, { 1440, 1400 } } do
		app:resize(size[1], size[2])
		app:frames(20)
		expect(app:error()).to_be(nil)
	end
end)

test("progress embed: crew rows' and board cards' lines without the host's rows", function()
	local app = mount("control_page_fixture.luau")
	app:resize(1440, 3200)
	app:frames(30)
	app:command("fixture.progress_none")
	app:frames(10)
	-- A card whose thread works: its line, from the session (its clock and menu).
	expect(app:has("#control-board >> #progress-strip-th-engine-state")).to_be(true)
	expect(app:has("#control-board >> #progress-strip-th-engine-state >> .orch-progress-menu")).to_be(true)
	-- One whose thread failed: none.
	expect(app:has("#control-board >> #progress-strip-th-ci-cache")).to_be(false)
end)

test("progress embed: a board card's line, pinned to the widget from its menu", function()
	local app = mount("control_page_fixture.luau")
	app:resize(1440, 3200)
	app:frames(30)
	local CARD = "#control-board >> #progress-strip-th-vpn-merge"
	expect(app:has(CARD)).to_be(true)
	app:click(`{CARD} >> .orch-progress-menu`)
	app:frames(10)
	app:click('role:menuitem "Pin to Progress"')
	app:frames(10)
	expect(table.concat(app:state("progress_pins"), ",")).to_be("th-vpn-merge")
	-- First in the widget.
	local first = app:tree("#control-widget-progress"):match("#progress%-full%-([%w%-]+) ")
	expect(first).to_be("th-vpn-merge")
end)

test("progress embed: a working session's line in its head", function()
	local app = mount("control_fixture.luau")
	app:resize(1440, 2400)
	app:frames(40)
	app:click("#progress-full-th-engine-state >> .orch-progress-menu")
	app:frames(10)
	app:click('role:menuitem "Open"')
	app:frames(30)
	expect(app:text("#page-title")).to_be("Engine: orchestration state")
	expect(app:has("#session-header >> #chat-progress >> #progress-strip-th-engine-state")).to_be(true)
	expect(app:has("#chat-progress >> ~\"5/8\"")).to_be(true)
	expect(app:has("#chat-progress >> ~\"last step 2m ago\"")).to_be(true)
	-- No row from its host: still a line, from its session.
	app:command("fixture.progress_none")
	app:frames(20)
	expect(app:has("#chat-progress >> #progress-strip-th-engine-state")).to_be(true)
	expect(app:has("#chat-progress >> ~\"cargo test -p zeron-engine\"")).to_be(true)
	expect(app:has("#chat-progress >> .orch-progress-cells")).to_be(false)
	-- At rest: no line.
	app:command("fixture.all_idle")
	app:frames(20)
	expect(app:has("#chat-progress")).to_be(false)
	for _, size in { { 900, 900 }, { 1440, 900 } } do
		app:resize(size[1], size[2])
		app:frames(20)
		-- (The fixture's host has no chat to open: that error is its own.)
		local e = app:error()
		expect(e == nil or string.find(tostring(e), "no host action named `chat.open`", 1, true) ~= nil).to_be(true)
	end
end)

-- The head's line is a crew thread's alone (its orchestrator's work being
-- followed): a chat of the person's own keeps its head as it was. And it's as
-- wide as what it says, centred: never its tool at one edge and its clocks
-- at the other.
local function box(app: any, target: string): (number, number)
	local line = string.match(app:tree(target), "[^\n]+") or ""
	local x, _, w = line:match(" (%-?%d+),(%-?%d+) (%d+)x(%d+)")
	return tonumber(x) or 0, tonumber(w) or 0
end

test("progress embed: a head's line is a crew thread's, as wide as what it says", function()
	local app = mount("control_fixture.luau")
	app:resize(1440, 2400)
	app:frames(40)
	app:click("#progress-full-th-engine-state >> .orch-progress-menu")
	app:frames(10)
	app:click('role:menuitem "Open"')
	app:frames(30)
	expect(app:has("#session-header >> #chat-progress >> #progress-strip-th-engine-state")).to_be(true)
	-- Hugging, centred in the head.
	app:command("fixture.progress_none")
	app:frames(20)
	local hx, hw = box(app, "#chat-progress")
	local sx, sw = box(app, "#progress-strip-th-engine-state")
	expect(sw < 560).to_be(true)
	expect(math.abs((sx + sw / 2) - (hx + hw / 2)) <= 2).to_be(true)
	-- Out of its crew: no line.
	app:command("fixture.uncrew")
	app:frames(20)
	expect(app:has("#chat-progress")).to_be(false)
end)
