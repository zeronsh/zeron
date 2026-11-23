-- A decision asked of the person (control_world's `fixture.decision`: two
-- variants with pictures and scores, a recommendation, context, links), in
-- the Control room: its question on its board card, its options as chips
-- in the card's detail and in Needs you; the detail's Open thread or the
-- thread's name opens the thread at the message that asked; the overlay
-- shows all of it,
-- and picking or replying answers it (account/orchestration's `answer`,
-- the option 1-based) once its Undo has passed. On ui/control_page_fixture.

local function open(): any
	local app = mount("control_page_fixture.luau")
	-- (Tall: the board under the rest, Needs you's Recent answers too.)
	app:resize(1440, 3600)
	app:frames(30)
	app:command("fixture.decision")
	app:frames(30)
	-- Where the engine puts its card: Needs you, asked, by that ask.
	local n = table.clone(app:state("orchestration") :: any)
	local cards = table.clone(n.cards or {})
	local now = os.time()
	table.insert(cards, { id = "it-topo", place = "needs_you", status = "asked", old_state = "active", since = now - 120, status_since = now - 120, asks = { "ak-topo" } })
	n.cards = cards
	app:set_state("orchestration", n)
	app:frames(30)
	return app
end

local function calls(app: any): string
	return tostring(app:state("calls") or "")
end

local CARD = '#control-board-tab >> .orch-card "Topology lines: pick a variant"'
-- (A card opened: its detail beside the board, its ask's own buttons.)
local DETAIL = "#control-board-tab >> #orch-detail-needs"
local function opened(app: any)
	app:click(CARD)
	app:frames(20)
end

-- To a tab of the Control room's (the Board's cards on its own).
local function tab(app: any, id: string)
	app:set_state("control_tab", id)
	app:frames(30)
end
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

test("a card waiting on a decision shows its question; opened, its options as chips", function()
	local app = open()
	tab(app, "board")
	expect(app:has(`{CARD} >> .orch-card-status >> "Waiting on you"`)).to_be(true)
	expect(app:has(`{CARD} >> .orch-card-summary >> "{QUESTION}"`)).to_be(true)
	-- (On the card itself, no chips: they're its detail's.)
	expect(app:has(`{CARD} >> .orch-choice`)).to_be(false)
	opened(app)
	-- The recommended one ringed and starred, named so; the others plain.
	expect(app:has(`{DETAIL} >> .orch-choice-recommended`)).to_be(true)
	expect(app:has(`{DETAIL} >> role:button "{SOLID}, recommended"`)).to_be(true)
	expect(app:has(`{DETAIL} >> role:button "{DASHED}"`)).to_be(true)
	expect(app:count(`{DETAIL} >> .orch-choice`)).to_be(3)
	expect(app:has(`{DETAIL} >> .orch-choice-expand`)).to_be(true)
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
	tab(app, "board")
	opened(app)
	app:click("#control-board-tab >> #orch-detail-ask-thread")
	app:frames(10)
	expect(calls(app)).to_contain("open:th-topo:msg-topo-ask")
end)

test("an ask's thread name opens the thread at the message that asked", function()
	local app = open()
	app:click('#control-widget-needs >> role:button "Open Topology lines at its question"')
	app:frames(10)
	expect(calls(app)).to_contain("open:th-topo:msg-topo-ask")
end)

test("a chip in the card's detail answers it, after its Undo", function()
	local app = open()
	tab(app, "board")
	opened(app)
	app:click(`{DETAIL} >> role:button "{DASHED}"`)
	app:frames(4)
	expect(app:has(`{DETAIL} >> .orch-sending >> "{DASHED}"`)).to_be(true)
	-- (Needs you says so too: one pick, wherever it's shown.)
	tab(app, "overview")
	expect(app:has('#control-widget-needs >> .orch-sending')).to_be(true)
	expect(calls(app)).never.to_contain("answer:")
	tab(app, "board")
	expect(app:wait(`{DETAIL} >> role:button "Undo"`, { gone = true, timeout_ms = 8000 })).to_be(true)
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
	-- Closed; on its way in Needs you, sent past its Undo.
	expect(closes(app)).to_be(true)
	expect(app:has("#control-widget-needs >> .orch-sending")).to_be(true)
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
	tab(app, "board")
	expect(app:has(`{CARD} >> "Deferred"`)).to_be(true)
	expect(app:has(`{CARD} >> .orch-choice`)).to_be(false)
end)

test("an older app sends an option and a reply as plain words: its label, a dash, the words", function()
	local app = overlay(open())
	app:command("fixture.answers_no_host")
	app:click(`.orch-decision >> .orch-decision-recommended >> "{SOLID}"`)
	app:frames(5)
	app:type('.orch-decision >> role:textbox "Reply"', "ship it today")
	app:key("Enter")
	app:frames(20)
	expect(app:wait('#control-widget-needs >> .orch-answered', { timeout_ms = 8000 })).to_be(true)
	expect(calls(app)).to_contain(`answer-op:ak-topo:{SOLID} — ship it today`)
	expect(app:error()).to_be_nil()
end)

test("a defer turned down is said on the ask, which stays; Retry sends it again", function()
	local app = overlay(open())
	app:command("fixture.answers_refuse")
	app:click('.orch-decision >> role:button "Defer"')
	app:frames(10)
	app:click('"For an hour"')
	app:frames(30)
	expect(closes(app)).to_be(true)
	expect(app:has(`#control-widget-needs >> role:button "{QUESTION}"`)).to_be(true)
	expect(app:has('#control-widget-needs >> "Couldn\'t defer it: That ask was already answered or dismissed"')).to_be(true)
	app:command("fixture.answers_ok")
	app:click('#control-widget-needs >> .orch-need >> role:button "Retry"')
	app:frames(30)
	local n = 0
	for _ in calls(app):gmatch("defer:ak%-topo:") do
		n += 1
	end
	expect(n).to_be(2)
	expect(app:has(`#control-widget-needs >> role:button "{QUESTION}"`)).to_be(false)
end)

test("an engine without deferAsk says so: the error is shown, nothing else tried", function()
	local app = overlay(open())
	app:command("fixture.answers_no_host")
	app:click('.orch-decision >> role:button "Defer"')
	app:frames(10)
	app:click('"For an hour"')
	app:frames(30)
	expect(calls(app)).to_contain("defer-op:ak-topo:")
	expect(app:has(`#control-widget-needs >> role:button "{QUESTION}"`)).to_be(true)
	expect(app:has('#control-widget-needs >> ~"Couldn\'t defer it: unknown variant `deferAsk`"')).to_be(true)
end)

test("the overlay stays open across the engine publishing its state again", function()
	local app = overlay(open())
	for _ = 1, 3 do
		app:command("fixture.republish")
		app:frames(10)
	end
	expect(shows(app, ".orch-decision")).to_be(true)
	-- And through a new ask arriving (the asks list changed under it).
	app:command("fixture.turn_end_ask")
	app:frames(30)
	expect(shows(app, ".orch-decision")).to_be(true)
end)

test("the overlay stays open after the click that opened it, and through a click inside", function()
	local app = open()
	tab(app, "board")
	app:click(`{CARD} >> .orch-choice-expand`)
	app:frames(30)
	expect(shows(app, ".orch-decision")).to_be(true)
	app:click(`.orch-decision >> role:button "{DASHED}"`)
	app:frames(30)
	expect(shows(app, ".orch-decision")).to_be(true)
	app:key("1")
	app:frames(30)
	expect(shows(app, ".orch-decision")).to_be(true)
end)

test("the overlay stays open when a tooltip or hover card was up as it opened", function()
	for _, from in { `{CARD} >> role:button "{DASHED}"`, `#control-widget-needs >> role:button "{DASHED}"`, `{CARD} >> .orch-card-title` } do
		local app = open()
		-- (The tab kept from the last time round: set each time.)
		tab(app, if string.find(from, "control-board-tab", 1, true) then "board" else "overview")
		app:hover(from)
		app:wait('role:dialog "Tooltip"', { timeout_ms = 2000 })
		app:frames(10)
		local expand = if string.find(from, "control-widget-needs", 1, true) then `#control-widget-needs >> role:button "{QUESTION}"` else `{CARD} >> .orch-choice-expand`
		app:click(expand)
		app:frames(30)
		expect(shows(app, ".orch-decision")).to_be(true)
		app:hover(".orch-decision")
		app:frames(60)
		app:wait('"never there"', { timeout_ms = 1500 })
		expect(shows(app, ".orch-decision")).to_be(true)
		app:key("2")
		app:frames(30)
		expect(shows(app, ".orch-decision")).to_be(true)
	end
end)

test("the overlay stays open while another ask in Needs you is answered", function()
	local app = overlay(open())
	app:command("fixture.answers_ok")
	-- (Ask ak-2, answered under it: its pick sent and taken, the list redrawn.)
	app:set_state("orch_decide_pending", { ["ak-2"] = { option = 1, label = "Approve", phase = "answered", sent = true, at = os.time() } })
	app:frames(30)
	expect(shows(app, ".orch-decision")).to_be(true)
	app:set_state("orch_decide_pending", { ["ak-2"] = { option = 1, label = "Approve", phase = "done", sent = true, at = os.time() } })
	app:frames(60)
	expect(shows(app, ".orch-decision")).to_be(true)
end)

test("the overlay stays open through a moment the engine's state lacks its ask", function()
	local app = overlay(open())
	app:command("fixture.asks_blip")
	app:frames(5)
	expect(app:has(`#control-widget-needs >> role:button "{QUESTION}"`)).to_be(false)
	-- (Past its closing's length: still up, its ask as it was.)
	app:wait('"never there"', { timeout_ms = 500 })
	app:frames(5)
	expect(shows(app, ".orch-decision")).to_be(true)
	expect(app:has(`.orch-decision >> ~"Topology lines: which variant ships"`)).to_be(true)
	app:wait('"never there"', { timeout_ms = 2500 })
	app:frames(20)
	expect(shows(app, ".orch-decision")).to_be(true)
end)
