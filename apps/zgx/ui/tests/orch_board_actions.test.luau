-- What the person does to a card on the board (ds/orch_board_actions), on
-- ui/orch_board_places_fixture.luau: each action calls its host prop with
-- the card's id (noted in `calls`), the fixture changes the rows as the
-- engine's next frame would, and the action is said in one toast. Set
-- `refuse`, the fixture rejects every action with those words: the danger
-- toast says them. A move the engine makes by itself (`engine_done`,
-- `engine_start`) is never said.

local DRAWERS = { snoozed = true, planned = true, earlier = true }

local function board(): any
	local app = mount("orch_board_places_fixture.luau")
	app:resize(1700, 1200)
	app:set_state("board_drawers", DRAWERS)
	app:frames(30)
	expect(app:error()).to_be_nil()
	return app
end

local function calls(app: any): string
	return app:state("calls") or ""
end

-- How many toasts were shown so far (one shown again with the same words
-- counts once: ds/toast restarts it in place).
local function shown(app: any): number
	return app:state("ds_toasts_next") or 0
end

-- The newest toast.
local function last(app: any): any
	local list = app:state("ds_toasts") or {}
	local newest = nil
	for _, t in list do
		if newest == nil or t.id > newest.id then
			newest = t
		end
	end
	return newest
end

-- `run` makes one toast with `title` (and `body`, when given), in `tone`.
local function says_once(app: any, title: string, body: string?, tone: string?, run: () -> ())
	-- (Those up cleared first: the same words again would restart one in
	-- place, not show another.)
	app:set_state("ds_toasts", {})
	app:frames(2)
	local before = shown(app)
	run()
	app:frames(10)
	expect(shown(app) - before).to_be(1)
	local t = last(app)
	expect(t.title).to_be(title)
	if body then
		expect(t.body).to_be(body)
	end
	expect(t.tone).to_be(tone or "neutral")
end

local function menu(app: any, card: string)
	app:right_click(`#orch-card-{card}`)
	app:frames(10)
end

local function pick(app: any, label: string)
	app:click(`role:menuitem "{label}"`)
	app:frames(20)
end

local function place_of(app: any, card: string): string?
	for _, c in app:state("fx_cards") or {} do
		if c.id == card then
			return c.place
		end
	end
	return nil
end

-- Each place's menu: what the person can do with a card as it stands.
local MENUS: { { card: string, has: { string }, hasnt: { string } } } = {
	{
		card = "it-merge",
		has = { "Open thread", "Pause", "Snooze", "Mark done", "Drop", "Delete from the board" },
		hasnt = { "Resume", "Start now", "Reopen", "Keep for later", "Let it start", "Bring back now" },
	},
	{
		card = "it-prs",
		has = { "Open thread", "Pause", "Bring back now", "Mark done", "Drop" },
		hasnt = { "Snooze", "Resume", "Start now", "Reopen" },
	},
	{
		card = "it-chat",
		has = { "Open thread", "Pause", "Mark done", "Drop", "Delete from the board" },
		hasnt = { "Snooze", "Bring back now", "Resume", "Start now", "Reopen", "Keep for later" },
	},
	{
		card = "it-keyed",
		has = { "Start now", "Pause", "Keep for later", "Mark done", "Drop" },
		hasnt = { "Open thread", "Resume", "Reopen", "Let it start", "Snooze" },
	},
	{
		card = "it-probes",
		has = { "Start now", "Let it start", "Mark done" },
		hasnt = { "Keep for later", "Resume", "Reopen" },
	},
	{
		card = "it-exits",
		has = { "Open thread", "Resume", "Mark done", "Drop" },
		hasnt = { "Pause", "Start now", "Reopen", "Keep for later", "Let it start" },
	},
	{
		card = "it-asks",
		has = { "Open thread", "Reopen", "Delete from the board" },
		hasnt = { "Mark done", "Drop", "Pause", "Resume", "Start now", "Snooze" },
	},
}

test("board actions: each card's menu offers what its place allows", function()
	for _, m in MENUS do
		-- (A fresh board each: a menu once opened stays mounted, hidden.)
		local app = board()
		menu(app, m.card)
		for _, label in m.has do
			expect(`{m.card}: {label} {app:has(`role:menuitem "{label}"`)}`).to_be(`{m.card}: {label} true`)
		end
		for _, label in m.hasnt do
			expect(`{m.card}: {label} {app:has(`role:menuitem "{label}"`)}`).to_be(`{m.card}: {label} false`)
		end
		expect(app:error()).to_be_nil()
	end
end)

test("board actions: Mark done closes it, said once", function()
	local app = board()
	says_once(app, "Marked done", "Chat list: only changed rows update", nil, function()
		menu(app, "it-chat")
		pick(app, "Mark done")
	end)
	expect(calls(app)).to_be("close:it-chat:false")
	expect(app:has("#orch-col-done >> #orch-card-it-chat")).to_be(true)
end)

test("board actions: Drop asks once more, then drops it", function()
	local app = board()
	menu(app, "it-chat")
	app:click('role:menuitem "Drop"')
	app:frames(30)
	-- (Armed: the entry asks, nothing is sent.)
	expect(calls(app)).to_be("")
	expect(app:tree('role:menuitem "Drop"')).to_contain('"?"')
	-- (Past the menu's guard against a double click.)
	app:wait('"never shown, only waited past"', { timeout_ms = 400 })
	says_once(app, "Dropped", "Chat list: only changed rows update", nil, function()
		app:click('role:menuitem "Drop"')
		app:frames(20)
	end)
	expect(calls(app)).to_be("close:it-chat:true")
	expect(place_of(app, "it-chat")).to_be("dropped")
end)

test("board actions: Reopen, from the menu and from the detail", function()
	local app = board()
	says_once(app, "Reopened", "Board cards on the orchestrator page answer asks", nil, function()
		menu(app, "it-asks")
		pick(app, "Reopen")
	end)
	expect(calls(app)).to_be("reopen:it-asks")
	expect(place_of(app, "it-asks")).to_be("planned")
	-- (A card from Earlier, from its detail.)
	app:click("#orch-card-it-old-1")
	app:frames(20)
	says_once(app, "Reopened", "Sidebar badge counts deferred asks", nil, function()
		app:click("#orch-detail-reopen")
	end)
	expect(calls(app)).to_be("reopen:it-asks,reopen:it-old-1")
end)

test("board actions: Keep for later and Let it start", function()
	local app = board()
	says_once(app, "Kept for later", "Keyed sidebar shape", nil, function()
		menu(app, "it-keyed")
		pick(app, "Keep for later")
	end)
	says_once(app, "Back in the queue", "Speed probes on the admin page", nil, function()
		menu(app, "it-probes")
		pick(app, "Let it start")
	end)
	expect(calls(app)).to_be("hold:it-keyed:true,hold:it-probes:false")
end)

test("board actions: Pause, then Resume from the card, the menu and the detail", function()
	local app = board()
	says_once(app, "Paused", "Chat list: only changed rows update", nil, function()
		menu(app, "it-chat")
		pick(app, "Pause")
	end)
	expect(place_of(app, "it-chat")).to_be("planned")
	-- (Paused: its card has Resume under its foot.)
	expect(app:has("#orch-card-it-chat >> .orch-card-resume")).to_be(true)
	says_once(app, "Resumed", "Chat list: only changed rows update", nil, function()
		app:click("#orch-card-it-chat >> .orch-card-resume")
	end)
	says_once(app, "Resumed", "Per-app exit preferences", nil, function()
		menu(app, "it-exits")
		pick(app, "Resume")
	end)
	expect(calls(app)).to_be("pause:it-chat,resume:it-chat,resume:it-exits")
	-- (From the detail, after pausing it again.)
	menu(app, "it-chat")
	pick(app, "Pause")
	app:click("#orch-card-it-chat")
	app:frames(20)
	says_once(app, "Resumed", "Chat list: only changed rows update", nil, function()
		app:click("#orch-detail-resume")
	end)
	expect(calls(app)).to_be("pause:it-chat,resume:it-chat,resume:it-exits,pause:it-chat,resume:it-chat")
end)

test("board actions: Start now, from the menu and from the detail", function()
	local app = board()
	says_once(app, "Started", "Keyed sidebar shape", nil, function()
		menu(app, "it-keyed")
		pick(app, "Start now")
	end)
	expect(place_of(app, "it-keyed")).to_be("running")
	app:click("#orch-card-it-win")
	app:frames(20)
	expect(app:has("#orch-detail-start")).to_be(true)
	says_once(app, "Started", "Windows installer smoke test", nil, function()
		app:click("#orch-detail-start")
	end)
	expect(calls(app)).to_be("start:it-keyed,start:it-win")
	-- (A card with a thread, or paused, has no Start now.)
	app:click("#orch-card-it-exits")
	app:frames(20)
	expect(app:has("#orch-detail-start")).to_be(false)
	expect(app:has("#orch-detail-resume")).to_be(true)
end)

test("board actions: Delete takes it off the board, its Undo puts it back", function()
	local app = board()
	says_once(app, "Deleted from the board", "Chat list: only changed rows update", nil, function()
		menu(app, "it-chat")
		pick(app, "Delete from the board")
	end)
	expect(app:has("#orch-card-it-chat")).to_be(false)
	local before = shown(app)
	app:click('role:button "Undo"')
	app:frames(20)
	expect(calls(app)).to_be("remove:it-chat,restore:it-chat")
	expect(app:has("#orch-card-it-chat")).to_be(true)
	-- (Put back: nothing more is said.)
	expect(shown(app)).to_be(before)
end)

test("board actions: Snooze, each of its four times, and Bring back now", function()
	for i, label in { "For an hour", "For 4 hours", "Until tomorrow, 9:00", "Until Monday, 9:00" } do
		local app = board()
		app:click("#orch-card-it-merge")
		app:frames(20)
		app:click("#orch-detail-snooze")
		app:frames(10)
		expect(app:has(`role:menuitem "{label}"`)).to_be(true)
		says_once(app, "Snoozed", "Bring main (0.3.8) into orchestrators", nil, function()
			app:click(`role:menuitem "{label}"`)
		end)
		expect(calls(app)).to_be("snooze:ak-push:later")
		expect(place_of(app, "it-merge")).to_be("snoozed")
		-- (Its time, as the fixture's clock gives the four.)
		local until_at = nil
		for _, a in app:state("fx_asks") do
			if a.id == "ak-push" then
				until_at = a.deferred_until
			end
		end
		expect(type(until_at)).to_be("number")
		if i == 1 then
			-- (Its detail now says when, and offers it back.)
			expect(app:has("#orch-detail-back-now")).to_be(true)
			says_once(app, "Brought back", "Bring main (0.3.8) into orchestrators", nil, function()
				app:click("#orch-detail-back-now")
			end)
			expect(calls(app)).to_be("snooze:ak-push:later,snooze:ak-push:now")
			expect(place_of(app, "it-merge")).to_be("needs_you")
		end
	end
end)

test("board actions: Snooze from the menu, Bring back now from a snoozed card's menu", function()
	local app = board()
	menu(app, "it-merge")
	app:hover('role:menuitem "Snooze"')
	app:frames(20)
	says_once(app, "Snoozed", nil, nil, function()
		app:click('role:menuitem "For an hour"')
	end)
	says_once(app, "Brought back", "12 pull requests to review", nil, function()
		menu(app, "it-prs")
		pick(app, "Bring back now")
	end)
	expect(calls(app)).to_be("snooze:ak-push:later,snooze:ak-prs:now")
end)

test("board actions: an answer from a card's detail is sent past its Undo, then said", function()
	local app = board()
	app:click("#orch-card-it-merge")
	app:frames(20)
	local before = shown(app)
	app:click('#orch-detail-needs >> role:button "Unlocked + push the fork"')
	app:frames(4)
	-- (Within its Undo: nothing sent, nothing said.)
	expect(calls(app)).to_be("")
	expect(app:wait('role:button "Undo"', { gone = true, timeout_ms = 8000 })).to_be(true)
	app:frames(10)
	expect(calls(app)).to_be("answer:ak-push:1:")
	expect(shown(app) - before).to_be(1)
	expect(last(app).title).to_be("Answered")
	expect(last(app).body).to_be("Bring main (0.3.8) into orchestrators")
	expect(place_of(app, "it-merge")).to_be("running")
end)

test("board actions: an answer from a list row", function()
	local app = board()
	app:set_state("board_view", "list")
	app:frames(30)
	local before = shown(app)
	app:click('#orch-list-needs_you >> role:button "Retry"')
	expect(app:wait('#orch-list >> role:button "Undo"', { gone = true, timeout_ms = 8000 })).to_be(true)
	app:frames(10)
	expect(calls(app)).to_be("answer:ak-failed:1:")
	expect(shown(app) - before).to_be(1)
	expect(last(app).title).to_be("Answered")
	expect(app:has("#orch-list-running >> #orch-row-it-admin")).to_be(true)
end)

test("board actions: refused, each says why in the danger's tone, and the card stays", function()
	local app = board()
	app:set_state("refuse", "The engine said no")
	app:frames(5)
	local cases: { { card: string, entry: string, title: string } } = {
		{ card = "it-chat", entry = "Mark done", title = "Couldn't do that" },
		{ card = "it-chat", entry = "Pause", title = "Couldn't pause it" },
		{ card = "it-exits", entry = "Resume", title = "Couldn't resume it" },
		{ card = "it-keyed", entry = "Start now", title = "Couldn't start it" },
		{ card = "it-keyed", entry = "Keep for later", title = "Couldn't do that" },
		{ card = "it-asks", entry = "Reopen", title = "Couldn't do that" },
		{ card = "it-prs", entry = "Bring back now", title = "Couldn't snooze it" },
		{ card = "it-chat", entry = "Delete from the board", title = "Couldn't delete it" },
	}
	for _, c in cases do
		local place = place_of(app, c.card)
		-- (The danger's toast stays until closed: closed after each.)
		app:set_state("ds_toasts", {})
		app:frames(5)
		says_once(app, c.title, "The engine said no", "danger", function()
			menu(app, c.card)
			pick(app, c.entry)
		end)
		expect(`{c.card} {place_of(app, c.card)}`).to_be(`{c.card} {place}`)
	end
	expect(app:has("#orch-card-it-chat")).to_be(true)
	expect(calls(app)).to_be(
		"close:it-chat:false,pause:it-chat,resume:it-exits,start:it-keyed,hold:it-keyed:true,reopen:it-asks,snooze:ak-prs:now,remove:it-chat"
	)
end)

test("board actions: a refused answer is said on the ask, with Retry, not in a toast", function()
	local app = board()
	app:set_state("refuse", "The engine said no")
	app:click("#orch-card-it-merge")
	app:frames(20)
	local before = shown(app)
	app:click('#orch-detail-needs >> role:button "Wait"')
	expect(app:wait('role:button "Undo"', { gone = true, timeout_ms = 8000 })).to_be(true)
	app:frames(10)
	expect(calls(app)).to_be("answer:ak-push:3:")
	expect(shown(app)).to_be(before)
	expect(app:has('#orch-detail-needs >> ~"The engine said no"')).to_be(true)
	expect(app:has('#orch-detail-needs >> role:button "Retry"')).to_be(true)
end)

test("board actions: a move the engine makes is never said", function()
	local app = board()
	local before = shown(app)
	app:command("engine_done")
	app:frames(20)
	app:command("engine_start")
	app:frames(20)
	expect(app:has("#orch-col-done >> #orch-card-it-chat")).to_be(true)
	expect(app:has("#orch-col-running >> #orch-card-it-keyed")).to_be(true)
	expect(shown(app)).to_be(before)
	expect(calls(app)).to_be("")
end)

test("board actions: the unseen dot is on Done cards only, and goes once the card is opened", function()
	local app = board()
	-- (it-asks and it-token were moved there by the engine and not opened.)
	expect(app:count(".orch-card-unseen")).to_be(2)
	expect(app:count("#orch-col-done >> .orch-card-unseen")).to_be(2)
	-- (An engine move to Running, unseen too, gets no dot.)
	app:command("engine_start")
	app:frames(20)
	expect(app:has("#orch-card-it-keyed >> .orch-card-unseen")).to_be(false)
	-- (An engine move to Done does.)
	app:command("engine_done")
	app:frames(20)
	expect(app:has("#orch-card-it-chat >> .orch-card-unseen")).to_be(true)
	app:click("#orch-card-it-asks")
	app:frames(20)
	expect(calls(app)).to_be("seen:it-asks")
	expect(app:has("#orch-card-it-asks >> .orch-card-unseen")).to_be(false)
	-- (One already seen sends nothing.)
	app:click("#orch-card-it-msi")
	app:frames(20)
	expect(calls(app)).to_be("seen:it-asks")
	-- (Opening it is not an action the person is told about.)
	expect(app:state("ds_toasts_next") or 0).to_be(0)
end)

test("board actions: New task is disabled while empty, sends the chosen orchestrator, and says so", function()
	local app = board()
	app:click("#orch-board-new")
	app:frames(30)
	expect(app:tree('role:button "Send"')).to_contain("disabled")
	app:type('#orch-new-task >> role:textbox', "   ")
	app:frames(5)
	expect(app:tree('role:button "Send"')).to_contain("disabled")
	app:type('#orch-new-task >> role:textbox', "Speed up the sidebar")
	app:frames(5)
	expect(app:tree('role:button "Send"')).never.to_contain("disabled")
	app:click('#orch-new-whose >> role:button "CI speed"')
	app:frames(5)
	says_once(app, "Task sent", "Speed up the sidebar", nil, function()
		app:click('role:button "Send"')
	end)
	expect(calls(app)).to_be("new:orc-ci:Speed up the sidebar")
	-- (Closed: kept mounted, hidden.)
	expect(app:tree('role:dialog "New task"', 1)).to_contain("hidden")
	-- (None chosen: whoever takes it.)
	app:click("#orch-board-new")
	app:frames(30)
	app:type('#orch-new-task >> role:textbox', "Look at the logs")
	app:frames(5)
	app:click('role:button "Send"')
	app:frames(10)
	expect(calls(app)).to_be("new:orc-ci:Speed up the sidebar,new::Look at the logs")
end)

test("board actions: New task, as the board's filter chose, and refused", function()
	local app = board()
	app:set_state("filter", "orc-vpn")
	app:frames(10)
	app:click("#orch-board-new")
	app:frames(30)
	app:type('#orch-new-task >> role:textbox', "Rotate the exit keys")
	app:frames(5)
	app:click('role:button "Send"')
	app:frames(10)
	expect(calls(app)).to_be("new:orc-vpn:Rotate the exit keys")
	app:set_state("refuse", "No Brain dump")
	app:click("#orch-board-new")
	app:frames(30)
	app:type('#orch-new-task >> role:textbox', "Again")
	app:frames(5)
	says_once(app, "Couldn't send the task", "No Brain dump", "danger", function()
		app:click('role:button "Send"')
	end)
end)

-- A card's detail: its Back (or close) only puts the detail away; Mark
-- done from its menu closes the card (`on_back` and `on_close` apart).
test("board actions: a detail's close leaves the card; Mark done in it closes it", function()
	local app = board()
	app:click("#orch-card-it-chat")
	app:frames(20)
	app:click("#orch-detail-close")
	app:frames(20)
	expect(app:has("#orch-board-detail")).to_be(false)
	expect(calls(app)).to_be("")
	expect(place_of(app, "it-chat")).to_be("running")
	app:click("#orch-card-it-chat")
	app:frames(20)
	app:click("#orch-detail-menu")
	app:frames(10)
	says_once(app, "Marked done", "Chat list: only changed rows update", nil, function()
		pick(app, "Mark done")
	end)
	expect(calls(app)).to_be("close:it-chat:false")
	expect(place_of(app, "it-chat")).to_be("done")
end)

test("board actions: in a panel's list, the detail's Back leaves the card; Mark done closes it", function()
	local app = mount("orch_board_places_fixture.luau")
	app:resize(900, 1000)
	app:set_state("only_list", true)
	app:set_state("compact", true)
	app:frames(30)
	app:click("#orch-row-it-chat")
	app:frames(20)
	expect(app:state("list_open")).to_be("it-chat")
	app:click("#orch-detail-back")
	app:frames(20)
	expect(app:state("list_open")).to_be_nil()
	expect(calls(app)).to_be("card:it-chat")
	expect(place_of(app, "it-chat")).to_be("running")
	app:click("#orch-row-it-chat")
	app:frames(20)
	app:click("#orch-detail-menu")
	app:frames(10)
	pick(app, "Mark done")
	expect(calls(app)).to_be("card:it-chat,card:it-chat,close:it-chat:false")
	expect(place_of(app, "it-chat")).to_be("done")
	-- (Closed, its detail stays open on it, now Done.)
	expect(app:state("list_open")).to_be("it-chat")
end)
