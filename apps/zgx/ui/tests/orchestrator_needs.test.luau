-- Needs you on an orchestrator's page is the Control room's (screens/
-- control/needs): the same rows, chips, dismiss, recent answers and
-- decision overlay, only that orchestrator's asks; an answer given in
-- either place is gone from both. On ui/orchestrator_fixture.luau.

local NEEDS = "#orch-section-needs"
local CONTROL = "#control-widget-needs"

local function settle(app: any, n: number?)
	app:frames(n or 30)
end

local function open(): any
	local app = mount("orchestrator_fixture.luau")
	app:resize(1440, 900)
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

local function calls(app: any): string
	return app:state("orch_calls") :: string
end

test("orchestrator needs: only its own asks", function()
	local app = open()
	-- Orchestrators' crew asks side by side or tabs; the VPN's reboot and
	-- CI speed's answered bucket question are other orchestrators'.
	expect(app:has(`{NEEDS} >> role:button "Side by side"`)).to_be(true)
	expect(app:has(`{NEEDS} >> ~"Reboot the Moscow node"`)).to_be(false)
	expect(app:has(`{NEEDS} >> ~"Which bucket should macOS runners use"`)).to_be(false)
	expect(app:count(`{NEEDS} >> .orch-need`)).to_be(1)
	-- The Control room has all of them.
	app:command("orch.control")
	settle(app)
	expect(app:has(`{CONTROL} >> role:button "Side by side"`)).to_be(true)
	expect(app:has(`{CONTROL} >> ~"Reboot the Moscow node"`)).to_be(true)
	expect(app:error()).to_be_nil()
end)

test("orchestrator needs: an ask expands into the decision, and closes again", function()
	local app = open()
	app:click(`{NEEDS} >> .orch-need-question`)
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

test("orchestrator needs: past six asks, n more expands the list and Fewer folds it", function()
	local app = open()
	-- (Tall enough for the eight and Fewer under them.)
	app:resize(1440, 2000)
	app:command("orch.many_asks")
	settle(app)
	expect(app:count(`{NEEDS} >> .orch-need`)).to_be(6)
	app:click(`{NEEDS} >> role:button "2 more"`)
	settle(app)
	expect(app:count(`{NEEDS} >> .orch-need`)).to_be(8)
	app:click(`{NEEDS} >> role:button "Fewer"`)
	settle(app)
	expect(app:count(`{NEEDS} >> .orch-need`)).to_be(6)
	expect(app:error()).to_be_nil()
end)

-- Answered: Undo for a moment, sent once, said, then out of the list.
local function answered_here(app: any, scope: string)
	app:click(`{scope} >> role:button "Side by side"`)
	expect(app:wait(`{scope} >> role:button "Undo"`, { gone = true, timeout_ms = 8000 })).to_be(true)
	expect(app:wait(`{scope} >> .orch-need >> "Side by side"`, { gone = true, timeout_ms = 6000 })).to_be(true)
	expect(calls(app)).to_contain("answer_ask:id=ak-1,option=1")
end

test("orchestrator needs: answered on its page, gone from the Control room too, and in its recent answers", function()
	local app = open()
	answered_here(app, NEEDS)
	settle(app)
	-- (Its thread still awaits input: a row to open it, no chips.)
	expect(app:has(`{NEEDS} >> role:button "Side by side"`)).to_be(false)
	app:click(`{NEEDS} >> role:button "Recent answers"`)
	settle(app, 6)
	expect(app:has(`{NEEDS} >> .orch-recent-row >> ~"Side by side"`)).to_be(true)
	-- Another orchestrator's answer isn't among its recent answers.
	expect(app:has(`{NEEDS} >> .orch-recent-row >> ~"macOS runners"`)).to_be(false)
	app:command("orch.control")
	settle(app)
	expect(app:has(`{CONTROL} >> .orch-need >> "Side by side"`)).to_be(false)
	expect(app:has(`{CONTROL} >> ~"Reboot the Moscow node"`)).to_be(true)
	expect(app:error()).to_be_nil()
end)

test("orchestrator needs: answered in the Control room, gone from its page", function()
	local app = open()
	app:command("orch.control")
	settle(app)
	answered_here(app, CONTROL)
	app:command("orch.orch")
	settle(app)
	expect(app:has(NEEDS)).to_be(true)
	expect(app:has(`{NEEDS} >> .orch-need >> "Side by side"`)).to_be(false)
	expect(app:error()).to_be_nil()
end)
