-- The Control room's Progress widget (screens/control/progress, on
-- ui/control_page_fixture.luau): every thread working now, the latest
-- started first, and those pinned before them (four, the rest folded);
-- pins kept on this device;
-- "What's up?" asks the host once (control_world notes it), then rests;
-- nothing working, one quiet line.

local W = "#control-widget-progress"

local function open(w: number?, h: number?): any
	local app = mount("control_page_fixture.luau")
	app:resize(w or 1440, h or 2400)
	app:frames(30)
	return app
end

-- The units listed, top to bottom.
local function listed(app: any): { string }
	local out = {}
	for line in app:tree(W):gmatch("[^\n]+") do
		local id = line:match("#progress%-full%-([%w%-]+) ")
		if id and not id:match("%-elapsed$") then
			table.insert(out, id)
		end
	end
	return out
end

test("control progress: a widget of its own, in the right column", function()
	local app = open()
	expect(app:has("#control-right >> #control-widget-progress")).to_be(true)
	expect(app:has(`{W} >> "Progress"`)).to_be(true)
	-- Its figures: how many work, how many are stalled.
	expect(app:has(`{W} >> ~"5 working"`)).to_be(true)
	expect(app:has(`{W} >> ~"1 stalled"`)).to_be(true)
	expect(app:error()).to_be(nil)
end)

test("control progress: what's working now, the latest started first", function()
	local app = open()
	-- The first four, the rest folded.
	expect(#listed(app)).to_be(4)
	expect(app:has('#control-progress-more >> "2 more"')).to_be(true)
	app:click("#control-progress-more")
	app:frames(10)
	expect(app:has('#control-progress-more >> "Fewer"')).to_be(true)
	local ids = listed(app)
	expect(table.concat(ids, ",")).to_be("orc-orch,th-vpn-merge,th-engine-state,th-ui-orch,th-engine-flow,th-ui-control")
	-- Done and failed aren't working.
	expect(table.find(ids, "th-vpn-moscow")).to_be(nil)
	expect(table.find(ids, "th-ci-cache")).to_be(nil)
	-- Where each one is: its orchestrator and machine.
	expect(app:has(`#progress-full-th-engine-state >> ~"Orchestrators · desk"`)).to_be(true)
	expect(app:has(`#progress-full-th-vpn-merge >> ~"Exnomic VPN · desk"`)).to_be(true)
end)

test("control progress: pinned threads first, kept, and unpinned", function()
	local app = open()
	app:set_state("progress_pins", { "th-vpn-moscow", "th-engine-flow" })
	app:set_state("progress_all", true)
	app:frames(10)
	local ids = listed(app)
	expect(ids[1]).to_be("th-vpn-moscow")
	expect(ids[2]).to_be("th-engine-flow")
	expect(ids[3]).to_be("orc-orch")
	-- (Once each.)
	expect(#ids).to_be(7)
	-- Unpinned from its own button: done, it goes.
	app:click("#progress-full-th-vpn-moscow >> .orch-progress-menu")
	app:frames(10)
	app:click('role:menuitem "Unpin from Progress"')
	app:frames(10)
	expect(table.find(listed(app), "th-vpn-moscow")).to_be(nil)
	expect(table.concat(app:state("progress_pins"), ",")).to_be("th-engine-flow")
	-- Pinned from a working one's: kept, first.
	app:click("#progress-full-th-ui-control >> .orch-progress-menu")
	app:frames(10)
	app:click('role:menuitem "Pin to Progress"')
	app:frames(10)
	expect(table.concat(app:state("progress_pins"), ",")).to_be("th-engine-flow,th-ui-control")
	expect(listed(app)[2]).to_be("th-ui-control")
end)

test("control progress: What's up? asks the host once, then rests", function()
	local app = open()
	local CHECK = "#progress-full-th-engine-state >> .orch-progress-check"
	app:click(CHECK)
	app:frames(5)
	expect(app:state("calls")).to_be("loads_wanted:on,check:th-engine-state")
	-- Its note on its way: the roller.
	expect(app:has("#progress-full-th-engine-state >> .orch-progress-refreshing")).to_be(true)
	app:click(CHECK)
	app:frames(5)
	expect(app:state("calls")).to_be("loads_wanted:on,check:th-engine-state")
	-- The fresh note in (the host's), and 15 s on: it can be asked again.
	expect(app:wait("#progress-full-th-engine-state >> ~\"Fresh:\"", { timeout_ms = 5000 })).to_be(true)
	expect(app:has("#progress-full-th-engine-state >> .orch-progress-refreshing")).to_be(false)
	app:frames(15 * 60 + 5)
	app:click(CHECK)
	app:frames(5)
	expect(app:state("calls")).to_be("loads_wanted:on,check:th-engine-state,check:th-engine-state")
end)

test("control progress: nothing working, one quiet line", function()
	local app = open()
	app:command("fixture.progress_none")
	app:frames(10)
	expect(#listed(app)).to_be(0)
	expect(app:has(`{W} >> .control-progress-empty >> ~"Nothing working"`)).to_be(true)
	expect(app:has(`{W} >> .orch-block-head >> ~"working"`)).to_be(false)
end)

test("control progress: drawn wide and narrow, resized", function()
	local app = open(1440, 2400)
	for _, size in { { 900, 2400 }, { 1440, 2400 }, { 700, 2600 }, { 1200, 2400 } } do
		app:resize(size[1], size[2])
		app:frames(20)
		expect(app:has(W)).to_be(true)
		expect(#listed(app)).to_be(4)
		expect(app:error()).to_be(nil)
	end
end)
