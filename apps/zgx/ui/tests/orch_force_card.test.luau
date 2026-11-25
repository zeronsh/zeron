-- The force Map's hover cards (ds/orch_force_card): a thread at work, one
-- done, an orchestrator, a machine; each action; and no transcript asked
-- for on hover (the host's calls untouched).

local function open(pivot: string?): any
	local app = mount("orch_map_fixture.luau")
	app:resize(1440, 2400)
	app:command("map_only")
	app:set_state("control_map_force_pivot", pivot or "orchestrator")
	app:command("show_all")
	app:command("card_progress")
	app:command("card_outcome")
	app:command("map_force")
	app:frames(90)
	app:wait(400)
	app:frames(20)
	return app
end
local function hover(app: any, id: string): string
	app:set_state("control_map_force_hover", id)
	app:frames(3)
	app:hover(".orch-force-anchor-box")
	expect(app:wait(".orch-force-card", { timeout_ms = 1500 })).to_be(true)
	-- (Shown once the pointer has rested: its delay, then its rise.)
	app:wait(700)
	app:frames(10)
	return app:tree(".orch-force-card") :: string
end
local function has(t: string, s: string): boolean
	return string.find(t, s, 1, true) ~= nil
end

test("force card: a thread at work: its whole title, state and time, its last step, its note, steps, where it sits", function()
	local app = open()
	local calls = tostring(app:state("calls"))
	local t = hover(app, "th-reins-win")
	expect(has(t, "Windows installer")).to_be(true)
	expect(has(t, "Working 14m")).to_be(true)
	expect(string.find(t, "last step %d+s ago") ~= nil).to_be(true)
	expect(has(t, "Signing the MSI with the release key")).to_be(true)
	-- (Its orchestrator, its card and its column.)
	expect(has(t, "Reins release")).to_be(true)
	expect(has(t, "Now")).to_be(true)
	-- No transcript asked for: the host's calls as they were.
	expect(tostring(app:state("calls"))).to_be(calls)
end)

test("force card: a thread done: its state and when, its card's outcome, unread, its pull request; Mark read", function()
	local app = open()
	local t = hover(app, "th-reins-sign")
	expect(has(t, "Code signing")).to_be(true)
	expect(has(t, "2h ago")).to_be(true)
	expect(has(t, "Signed the installer with the new key")).to_be(true)
	expect(has(t, "Unread")).to_be(true)
	expect(has(t, "#412")).to_be(true)
	app:click("#orch-force-card-read")
	app:frames(10)
	expect((app:state("control_map_dismissed") or {})["th-reins-sign"] ~= nil).to_be(true)
end)

test("force card: an orchestrator's crew and board tallies; Open page", function()
	local app = open()
	local t = hover(app, "orc-reins")
	-- (Each fact its own words, spaced apart: no typed separator.)
	for _, w in { "Crew", "2 working", "1 idle", "0 failed", "Board", "2 now", "0 in review", "0 blocked" } do
		expect(`{w} {has(t, w)}`).to_be(`{w} true`)
	end
	expect(string.find(t, "·", 1, true)).to_be(nil)
	app:click("#orch-force-card-open-page")
	app:frames(5)
	expect(string.find(tostring(app:state("shell_route_path")), "orc-reins", 1, true) ~= nil).to_be(true)
end)

test("force card: a machine on the machines pivot: online, how many work on it", function()
	local app = open("machine")
	local t = hover(app, "machine:dev-forge")
	expect(has(t, "Online")).to_be(true)
	expect(has(t, "working")).to_be(true)
	app:set_state("control_map_force_pivot", "orchestrator")
	app:frames(5)
end)

test("force card: Open chat, Open card, Pin and Unpin", function()
	local app = open()
	hover(app, "th-reins-win")
	app:click("#orch-force-card-pin")
	app:frames(5)
	expect((app:state("control_map_force_pins") or {})["th-reins-win"]).to_be(true)
	-- (Its card again, the pin's word now Unpin.)
	local t = hover(app, "th-reins-win")
	expect(string.find(t, "Unpin", 1, true) ~= nil).to_be(true)
	app:click("#orch-force-card-pin")
	app:frames(5)
	expect((app:state("control_map_force_pins") or {})["th-reins-win"]).to_be(nil)
	hover(app, "th-reins-win")
	app:click("#orch-force-card-open-card")
	app:frames(5)
	expect(app:state("control_tab")).to_be("board")
	expect(app:state("control_board_reveal")).to_be("it-win")
	app:set_state("control_tab", "map")
	-- (Settled first: Needs you floats in again, the Map's room changes
	-- with it and the Graph is fitted to it once more.)
	app:frames(10)
	app:wait(400)
	app:frames(10)
	hover(app, "th-reins-win")
	app:click("#orch-force-card-open")
	app:frames(5)
	expect(app:state("shell_route_path")).to_be("/session/th-reins-win")
end)

-- Hover intent: the card stays while the pointer crosses from the node to
-- it and while it's on it; off all of them, it goes after a moment, and at
-- once where the pointer is clearly elsewhere (tests/orch_force_hover).
-- Both pivots.
-- (The pointer on the field by hand, at a node's middle moved by dx, dy.)
local function pointer_at(app: any, id: string, dx: number, dy: number)
	app:set_state("force_ptr", { op = "move", id = id, dx = dx, dy = dy })
	app:command("map_force_pointer")
end
for _, pv in { "orchestrator", "machine" } do
	test(`force card ({pv}): node, gap, card: the card stays; off everything it goes after a moment`, function()
		local app = open(pv)
		hover(app, "th-reins-win")
		-- The gap (the field between the node and its card, below it): still
		-- there a moment on.
		pointer_at(app, "th-reins-win", 0, 22)
		app:frames(3)
		expect(app:state("control_map_force_hover")).to_be("th-reins-win")
		-- On the card: kept, however long.
		app:hover(".orch-force-card")
		app:wait(700)
		app:frames(5)
		expect(app:state("control_map_force_hover")).to_be("th-reins-win")
		expect(app:has(".orch-force-card")).to_be(true)
		-- Its buttons fire: Pin.
		app:click("#orch-force-card-pin")
		app:frames(5)
		expect((app:state(if pv == "machine" then "control_map_force_pins_machine" else "control_map_force_pins") or {})["th-reins-win"]).to_be(true)
		-- Off everything: gone after a moment.
		app:hover(".orch-force-legend")
		app:frames(2)
		app:wait(450)
		app:frames(5)
		expect(app:state("control_map_force_hover")).to_be(nil)
		app:set_state("control_map_force_pivot", "orchestrator")
		app:frames(5)
	end)
end

test("force card: the pin over a lit pinned node is a button: on the way, kept; clicked, unpinned; no drag, no pan", function()
	local app = open()
	hover(app, "th-reins-win")
	app:click("#orch-force-card-pin")
	app:frames(5)
	expect((app:state("control_map_force_pins") or {})["th-reins-win"]).to_be(true)
	hover(app, "th-reins-win")
	expect(app:has("#orch-force-pin")).to_be(true)
	-- On the pin: kept.
	app:hover("#orch-force-pin")
	app:wait(500)
	app:frames(3)
	expect(app:state("control_map_force_hover")).to_be("th-reins-win")
	local views = app:state("control_map_force_views")
	app:click("#orch-force-pin")
	app:frames(10)
	expect((app:state("control_map_force_pins") or {})["th-reins-win"]).to_be(nil)
	-- (A press on the card or the pin pans nothing.)
	hover(app, "th-reins-win")
	app:click(".orch-force-card-state")
	app:frames(5)
	local after = app:state("control_map_force_views")
	expect(((after or {}).orchestrator or {}).x).to_be(((views or {}).orchestrator or {}).x)
end)

test("force card: a thread whose card is paused: Resume resumes its card, into Now", function()
	local app = open()
	local t0 = hover(app, "th-reins-sign")
	expect(has(t0, '"Resume"')).to_be(false)
	app:set_state("control_map_force_hover", nil)
	app:command("sign_paused")
	app:frames(10)
	local t = hover(app, "th-reins-sign")
	expect(has(t, "Resume")).to_be(true)
	app:click("#orch-force-card-resume")
	app:frames(10)
	expect(tostring(app:state("calls"))).to_contain("op:resumeItem:it-sign")
end)
