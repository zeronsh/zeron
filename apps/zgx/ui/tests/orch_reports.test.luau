-- Crew updates (ds/orch_reports): a row a report, three at first and the
-- rest under "Show N more", a row opening its thread; in a transcript, the
-- engine's crew reports message is that card, never a bubble of the
-- person's (ui/orch_reports_fixture.luau).

local CARD = "#card-side"

local function rows(app: any, within: string): number
	return app:count(`{within} >> .orch-report`)
end

test("crew updates: three rows, then all of them, then three again", function()
	local app = mount("orch_reports_fixture.luau")
	app:resize(1200, 800)
	app:frames(10)
	expect(app:has(`{CARD} >> ~"Crew updates"`)).to_be(true)
	expect(rows(app, CARD)).to_be(3)
	expect(app:has(`{CARD} >> ~"Engine: orchestration state"`)).to_be(true)
	expect(app:has(`{CARD} >> ~"Needs a decision: tabs or split"`)).to_be(true)
	expect(app:has(`{CARD} >> ~"Map widget lands"`)).to_be(false)
	-- Each state's chip.
	expect(app:has(`{CARD} >> ~"Blocked"`)).to_be(true)
	expect(app:has(`{CARD} >> ~"Update"`)).to_be(true)
	app:click(`{CARD} >> ~"Show 2 more"`)
	app:frames(10)
	expect(rows(app, CARD)).to_be(5)
	expect(app:has(`{CARD} >> ~"Map widget lands"`)).to_be(true)
	expect(app:has(`{CARD} >> ~"Failed"`)).to_be(true)
	expect(app:has(`{CARD} >> ~"Done"`)).to_be(true)
	app:click(`{CARD} >> ~"Show less"`)
	app:frames(10)
	expect(rows(app, CARD)).to_be(3)
	expect(app:has(`{CARD} >> ~"Show 2 more"`)).to_be(true)
end)

test("crew updates: a row opens its thread", function()
	local app = mount("orch_reports_fixture.luau")
	app:resize(1200, 800)
	app:frames(10)
	app:click(`{CARD} >> ~"CrewReports part renders"`)
	app:frames(2)
	expect(app:state("opened")).to_be("th-engine-flow")
	app:click(`{CARD} >> ~"Needs a decision"`)
	app:frames(2)
	expect(app:state("opened")).to_be("th-ui-orch")
end)

test("crew updates: new reports join in place, the fold kept", function()
	local app = mount("orch_reports_fixture.luau")
	app:resize(1200, 800)
	app:frames(10)
	local now = app:state("reports") :: { any }
	local more = table.clone(now)
	table.insert(more, { id = "rp-x3", from_chat = "th-ui-orch", state = "done", summary = "Crew updates card done", at = os.time() })
	app:set_state("reports", more)
	app:frames(10)
	expect(rows(app, CARD)).to_be(3)
	expect(app:has(`{CARD} >> ~"Show 3 more"`)).to_be(true)
end)

test("crew updates: in a transcript, a card and never the person's bubble", function()
	local app = mount("orch_reports_fixture.luau")
	app:resize(1200, 800)
	app:frames(20)
	expect(app:has("#conversation >> .message-crew-reports")).to_be(true)
	expect(app:count("#conversation >> .message-user")).to_be(1)
	expect(app:has("#conversation >> .message-user >> ~\"Needs a decision\"")).to_be(false)
	expect(app:count("#conversation >> .message-crew-reports >> .orch-report")).to_be(3)
	app:click("#conversation >> .message-crew-reports >> ~\"Needs a decision\"")
	app:frames(2)
	expect(app:state("opened")).to_be("zeron-chat:th-ui-orch")
end)

test("crew updates: narrow and wide, drawn and resized", function()
	local app = mount("orch_reports_fixture.luau")
	for _, size in { { 420, 700 }, { 1200, 800 }, { 760, 600 }, { 420, 900 } } do
		app:resize(size[1], size[2])
		app:frames(10)
		expect(rows(app, CARD)).to_be(3)
		expect(app:error()).to_be(nil)
	end
end)
