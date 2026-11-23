-- Needs you reached from an orchestrator's page: the Control room's
-- widget (screens/control/needs), every orchestrator's rows, its decision
-- overlay, its "n more". (The page's own Needs you section and its Board
-- are gone: its side panel's Tasks list shows its cards, tested in
-- tests/orch_side_panel.) On ui/orchestrator_fixture.luau.

local CONTROL = "#control-widget-needs"

local function settle(app: any, n: number?)
	app:frames(n or 30)
end

local function open(): any
	local app = mount("orchestrator_fixture.luau")
	app:resize(1440, 900)
	settle(app)
	app:command("orch.control")
	settle(app)
	return app
end

-- Whether a target shows (a closed modal's nodes stay, hidden).
local function shows(app: any, target: string): boolean
	if not app:has(target) then
		return false
	end
	local first = string.match(app:tree(target, 0), "^[^\n]*") or ""
	return not string.find(first, "hidden", 1, true)
end

local function closes(app: any): boolean
	for _ = 1, 40 do
		app:frames(5)
		if not shows(app, ".orch-decision") then
			return true
		end
	end
	return false
end

test("orchestrator needs: the Control room has every orchestrator's", function()
	local app = open()
	-- Orchestrators' card (side by side or tabs) and the VPN's reboot; CI
	-- speed's bucket question is answered.
	expect(app:has(`{CONTROL} >> role:button "Side by side"`)).to_be(true)
	expect(app:has(`{CONTROL} >> ~"Reboot the Moscow node"`)).to_be(true)
	expect(app:has(`{CONTROL} >> ~"Which bucket should macOS runners use"`)).to_be(false)
	expect(app:error()).to_be_nil()
end)

test("orchestrator needs: an ask expands into the decision, and closes again", function()
	local app = open()
	app:click(`{CONTROL} >> .orch-need-question ~"side by side, or tabs?"`)
	settle(app)
	expect(app:state("orch_decide_shown")).to_be("ak-1")
	expect(shows(app, ".orch-decision")).to_be(true)
	expect(app:has('.orch-decision >> ~"side by side, or tabs?"')).to_be(true)
	app:key("Escape")
	settle(app)
	expect(app:state("orch_decide_shown")).to_be_nil()
	expect(closes(app)).to_be(true)
	expect(app:error()).to_be_nil()
end)

test("orchestrator needs: past six rows, n more expands the list and Fewer folds it", function()
	local app = open()
	-- (Tall enough for every row and Fewer under them.)
	app:resize(1440, 2400)
	app:command("orch.many_asks")
	settle(app)
	-- The sample's three Needs-you cards and the seven asks of a Running
	-- card's thread (none of them a Needs-you card's): ten.
	expect(app:count(`{CONTROL} >> .orch-need`)).to_be(6)
	app:click(`{CONTROL} >> role:button "4 more"`)
	settle(app)
	expect(app:count(`{CONTROL} >> .orch-need`)).to_be(10)
	app:click(`{CONTROL} >> role:button "Fewer"`)
	settle(app)
	expect(app:count(`{CONTROL} >> .orch-need`)).to_be(6)
	expect(app:error()).to_be_nil()
end)
