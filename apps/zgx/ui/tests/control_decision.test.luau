-- A decision asked of the person (control_world's `fixture.decision`: two
-- variants with pictures and scores, a recommendation, context, links), in
-- the Control room: compact on its board card and in Needs you (its
-- question in a line, its options as chips); a card or the thread's name
-- opens the thread at the message that asked; the overlay shows all of it,
-- and picking or replying answers it (account/orchestration's `answer`,
-- the option 1-based) once its Undo has passed. On ui/control_page_fixture.

local function open(): any
	local app = mount("control_page_fixture.luau")
	-- (Tall: the board under the rest.)
	app:resize(1440, 3200)
	app:frames(30)
	app:command("fixture.decision")
	app:frames(30)
	return app
end

local function calls(app: any): string
	return tostring(app:state("calls") or "")
end

local CARD = '#control-widget-board >> .orch-card "Topology lines: pick a variant"'
local QUESTION = "Topology lines: which variant ships, solid lead lines or dashed crew lines?"
local SOLID = "Solid lead lines"
local DASHED = "Dashed crew lines"

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

local function overlay(app: any): any
	app:click(`#control-widget-needs >> role:button "{QUESTION}"`)
	app:frames(30)
	expect(shows(app, ".orch-decision")).to_be(true)
	return app
end

test("a card waiting on a decision shows it compact: the question in a line, its options as chips", function()
	local app = open()
	expect(app:has(`{CARD} >> "Waiting on you"`)).to_be(true)
	expect(app:has(`{CARD} >> .orch-card-question`)).to_be(true)
	-- The recommended one ringed and starred, named so; the others plain.
	expect(app:has(`{CARD} >> .orch-choice-recommended`)).to_be(true)
	expect(app:has(`{CARD} >> role:button "{SOLID}, recommended"`)).to_be(true)
	expect(app:has(`{CARD} >> role:button "{DASHED}"`)).to_be(true)
	expect(app:count(`{CARD} >> .orch-choice`)).to_be(3)
	expect(app:has(`{CARD} >> .orch-choice-expand`)).to_be(true)
	expect(app:error()).to_be_nil()
end)

test("Needs you shows it compact too, chips and all", function()
	local app = open()
	expect(app:has(`#control-widget-needs >> role:button "{QUESTION}"`)).to_be(true)
	expect(app:has(`#control-widget-needs >> role:button "{SOLID}, recommended"`)).to_be(true)
	expect(app:has(`#control-widget-needs >> role:button "{DASHED}"`)).to_be(true)
end)

test("a card waiting on a decision opens its thread at the message that asked", function()
	local app = open()
	app:click(`{CARD} >> .orch-card-title`)
	app:frames(10)
	expect(calls(app)).to_contain("open:th-topo:msg-topo-ask")
end)

test("an ask's thread name opens the thread at the message that asked", function()
	local app = open()
	app:click('#control-widget-needs >> role:button "Open Topology lines at its question"')
	app:frames(10)
	expect(calls(app)).to_contain("open:th-topo:msg-topo-ask")
end)

test("a chip on the card answers it, after its Undo", function()
	local app = open()
	app:click(`{CARD} >> role:button "{DASHED}"`)
	app:frames(4)
	expect(app:has(`{CARD} >> .orch-answered >> "{DASHED}"`)).to_be(true)
	-- (Needs you says so too: one pick, wherever it's shown.)
	expect(app:has('#control-widget-needs >> .orch-answered')).to_be(true)
	expect(calls(app)).never.to_contain("answer:")
	expect(app:wait(`{CARD} >> role:button "Undo"`, { gone = true, timeout_ms = 8000 })).to_be(true)
	app:frames(4)
	expect(calls(app)).to_contain("answer:ak-topo:2:")
end)

test("the overlay shows the question, its context, the options with their pictures, scores and the recommendation", function()
	local app = overlay(open())
	local O = ".orch-decision"
	expect(app:has(`{O} >> .orch-decision-question >> ~"Topology lines: which variant ships"`)).to_be(true)
	-- Whose, whose orchestrator, when; its links.
	expect(app:has(`{O} >> role:button "Open Topology lines at its question"`)).to_be(true)
	expect(app:has(`{O} >> ~"asked 8h ago"`)).to_be(true)
	expect(app:has(`{O} >> .orch-decision-link`)).to_be(true)
	-- The context, folding.
	expect(app:has(`{O} >> .orch-decision-context >> ~"Both are built on"`)).to_be(true)
	app:click(`{O} >> .orch-decision-context-toggle`)
	app:frames(10)
	expect(app:has(`{O} >> ~"Both are built on"`)).to_be(false)
	-- The options: three cards, two with their picture, scores, and the
	-- recommended one saying why.
	expect(app:count(`{O} >> .orch-decision-option`)).to_be(3)
	expect(app:count(`{O} >> .orch-decision-picture`)).to_be(2)
	expect(app:count(`{O} >> .orch-decision-score`)).to_be(6)
	expect(app:has(`{O} >> .orch-decision-recommended >> .orch-decision-why >> ~"Reads at a glance"`)).to_be(true)
	expect(app:count(`{O} >> .orch-decision-why`)).to_be(1)
	expect(app:error()).to_be_nil()
end)

test("a picture in the overlay opens the lightbox", function()
	local app = overlay(open())
	app:click('.orch-decision >> role:button "Open topology-a.png"')
	app:frames(60)
	local shown = app:state("ds/lightbox.luau::open")
	expect(shown ~= nil).to_be(true)
	expect(shown.items[1].name).to_be("topology-a.png")
end)

test("in the overlay, a number chooses an option and Decide answers with it", function()
	local app = overlay(open())
	app:key("2")
	app:frames(5)
	expect(app:has(`.orch-decision >> role:button "{DASHED}"`)).to_be(true)
	app:click('.orch-decision >> role:button "Decide"')
	app:frames(20)
	-- Closed; answered in Needs you, sent past its Undo.
	expect(closes(app)).to_be(true)
	expect(app:has("#control-widget-needs >> .orch-answered")).to_be(true)
	expect(app:wait('#control-widget-needs >> role:button "Undo"', { gone = true, timeout_ms = 8000 })).to_be(true)
	app:frames(4)
	expect(calls(app)).to_contain("answer:ak-topo:2:")
end)

test("in the overlay, a click chooses, and the reply goes with it on Enter", function()
	local app = overlay(open())
	-- (On its name: its picture opens the lightbox.)
	app:click(`.orch-decision >> .orch-decision-recommended >> "{SOLID}"`)
	app:frames(5)
	app:type('.orch-decision >> role:textbox "Reply"', "ship it today")
	app:key("Enter")
	app:frames(20)
	expect(closes(app)).to_be(true)
	expect(app:wait('#control-widget-needs >> role:button "Undo"', { gone = true, timeout_ms = 8000 })).to_be(true)
	app:frames(4)
	expect(calls(app)).to_contain("answer:ak-topo:1:ship it today")
end)

test("in the overlay, a reply alone answers it", function()
	local app = overlay(open())
	app:type('.orch-decision >> role:textbox "Reply"', "Neither: try 3 first")
	app:key("Enter")
	app:frames(20)
	expect(app:wait('#control-widget-needs >> role:button "Undo"', { gone = true, timeout_ms = 8000 })).to_be(true)
	app:frames(4)
	expect(calls(app)).to_contain("answer:ak-topo::Neither: try 3 first")
end)

test("Escape closes the overlay, nothing answered", function()
	local app = overlay(open())
	app:key("Escape")
	app:frames(30)
	expect(closes(app)).to_be(true)
	expect(calls(app)).never.to_contain("answer:")
end)

test("Defer puts it off: out of Needs you, its card says Deferred", function()
	local app = overlay(open())
	app:click('.orch-decision >> role:button "Defer"')
	app:frames(10)
	app:click('"For an hour"')
	app:frames(30)
	expect(calls(app)).to_contain("defer:ak-topo:")
	expect(closes(app)).to_be(true)
	expect(app:has(`#control-widget-needs >> role:button "{QUESTION}"`)).to_be(false)
	expect(app:has(`{CARD} >> "Deferred"`)).to_be(true)
	expect(app:has(`{CARD} >> .orch-choice`)).to_be(false)
end)
