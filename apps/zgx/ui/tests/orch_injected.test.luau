-- What the engine says into a conversation (ds/orch_injected), in the
-- orchestrator page's (ui/injected_blocks_fixture.luau): each a quiet
-- block, never the person's bubble; what's only for the agent never shows;
-- a crew ask says how its ask stands; a crew ask an older engine wrote
-- (its text alone) reads the same.

local CONV = "#orch-conversation"

local function mounted(): any
	local app = mount("injected_blocks_fixture.luau")
	app:resize(1400, 2400)
	app:frames(30)
	return app
end

test("injected: a block each, the agent's words never shown", function()
	local app = mounted()
	expect(app:count(`{CONV} >> .orch-injected-crew_ask`)).to_be(4)
	expect(app:count(`{CONV} >> .orch-injected-answer`)).to_be(2)
	for _, kind in { "for_thread", "board_check", "resumed", "scheduled", "on_card", "changes", "retraction" } do
		expect(app:count(`{CONV} >> .orch-injected-{kind}`)).to_be(1)
	end
	-- Nothing that's only the agent's: no fences, no instructions, no
	-- headers.
	for _, raw in { "crew-output", "\u{27E6}", "answer_ask", "[Crew ask]", "[Answer from", "[Dump", "Route this; don't act on it", "`world`", "[Crew rules from", "[For your crew thread", "[On your card", "[Changes requested", "[Board check]", "[Resumed]", "[Brain dump]", "[Quoted message", "item_upsert", "untrusted output" } do
		expect(app:has(`{CONV} >> ~"{raw}"`)).to_be(false)
	end
	-- The words that are the person's or a sender's stay, as bubbles.
	expect(app:has(`{CONV} >> ~"remind me to rerun omarchy update"`)).to_be(true)
	expect(app:has(`{CONV} >> ~"The Moscow node is back"`)).to_be(true)
	expect(app:has(`{CONV} >> ~"Take the map lanes next"`)).to_be(true)
end)

test("injected: one card for an ask, open, deferred or answered", function()
	local app = mounted()
	expect(app:count(`{CONV} >> .orch-ask-state`)).to_be(6)
	expect(app:has(`{CONV} >> ~"Answered by orchestrator"`)).to_be(true)
	expect(app:has(`{CONV} >> ~"Deferred"`)).to_be(true)
	expect(app:has(`{CONV} >> ~"The panel, so the thread stays beside the report"`)).to_be(true)
	-- The option picked, lit; the others beside it.
	expect(app:count(`{CONV} >> .orch-ask-picked`)).to_be(2)
	expect(app:has(`{CONV} >> ~"Not yet"`)).to_be(true)
	-- An older engine's crew ask and answer, read from their text.
	expect(app:has(`{CONV} >> ~"creating an orchestrator from a flow"`)).to_be(true)
	expect(app:has(`{CONV} >> ~"Signed in"`)).to_be(true)
	expect(app:has(`{CONV} >> ~"[Answer from you]"`)).to_be(false)
end)
