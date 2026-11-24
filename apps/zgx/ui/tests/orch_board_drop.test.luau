-- A card dropped on the board (ds/orch_board_drop), on
-- ui/orch_board_places_fixture.luau: each drop the board takes calls the
-- host prop its card's menu calls there (noted in `calls`), the fixture
-- moves the card as the engine's next frame would, and the action is said
-- in one toast. A drop the board refuses, or one on the card's own place,
-- calls and says nothing. A card's drag is still its reference, for a
-- composer (tests/control_drag_refs.test.luau drags one into Capture).

local function board(drawers: { [string]: boolean }?): any
	local app = mount("orch_board_places_fixture.luau")
	app:resize(1700, 1200)
	app:set_state("board_drawers", drawers or {})
	app:frames(30)
	expect(app:error()).to_be_nil()
	return app
end
local PLANNED = { planned = true }
local ALL = { snoozed = true, planned = true, earlier = true }

-- Each target: a column (by its head, in it) or Planned's drawer.
local ONTO = {
	needs_you = "#orch-col-needs_you >> .orch-column-head",
	running = "#orch-col-running >> .orch-column-head",
	planned = "#orch-drawer-planned >> .orch-drawer-head",
	done = "#orch-col-done >> .orch-column-head",
}

local function calls(app: any): string
	return app:state("calls") or ""
end
local function shown(app: any): number
	return app:state("ds_toasts_next") or 0
end
local function last(app: any): any
	local newest = nil
	for _, t in app:state("ds_toasts") or {} do
		if newest == nil or t.id > newest.id then
			newest = t
		end
	end
	return newest
end
local function card_of(app: any, id: string): any
	for _, c in app:state("fx_cards") or {} do
		if c.id == id then
			return c
		end
	end
	return nil
end

-- (Then what it moved has landed: its hold and travel, ds/orch_board_motion.)
local function drop(app: any, id: string, onto: string)
	app:drag(`#orch-card-{id}`, ONTO[onto])
	app:frames(75)
end

-- `id` dropped on `onto` calls `call` and says `title` (its title under it).
local function takes(app: any, id: string, onto: string, call: string, title: string, place: string)
	app:set_state("ds_toasts", {})
	app:set_state("calls", "")
	app:frames(2)
	local before = shown(app)
	drop(app, id, onto)
	expect(calls(app)).to_be(call)
	expect(shown(app) - before).to_be(1)
	local t = last(app)
	expect(t.title).to_be(title)
	expect(t.tone).to_be("success")
	expect(card_of(app, id).place).to_be(place)
	-- (Let go: nothing carried any more.)
	expect(app:has(".orch-drop-lit")).to_be(false)
	expect(app:has(".orch-drop-badge")).to_be(false)
end

-- `id` dropped on `onto` does and says nothing: it stays where it was.
local function refuses(app: any, id: string, onto: string)
	app:set_state("ds_toasts", {})
	app:set_state("calls", "")
	app:frames(2)
	local before, place = shown(app), card_of(app, id).place
	drop(app, id, onto)
	expect(calls(app)).to_be("")
	expect(shown(app) - before).to_be(0)
	expect(card_of(app, id).place).to_be(place)
	expect(app:has(`#orch-card-{id}`)).to_be(true)
end

test("board drop: a running card on Planned pauses it", function()
	local app = board()
	takes(app, "it-chat", "planned", "pause:it-chat", "Paused", "planned")
	expect(card_of(app, "it-chat").status).to_be("paused")
end)

test("board drop: a paused card on Running resumes it", function()
	local app = board(PLANNED)
	takes(app, "it-exits", "running", "resume:it-exits", "Resumed", "running")
end)

test("board drop: a card not started on Running starts it", function()
	local app = board(PLANNED)
	takes(app, "it-keyed", "running", "start:it-keyed", "Started", "running")
	-- (One held with no thread yet, the same.)
	takes(app, "it-probes", "running", "start:it-probes", "Started", "running")
end)

test("board drop: a done card on Planned reopens it", function()
	local app = board()
	takes(app, "it-asks", "planned", "reopen:it-asks", "Reopened", "planned")
end)

test("board drop: a done card on Running reopens and starts it, said as both", function()
	local app = board()
	takes(app, "it-asks", "running", "resume:it-asks", "Reopened and started", "running")
	-- (Its timeline's line, from the engine: one move, both said.)
	expect(card_of(app, "it-asks").reason).to_be("You reopened and started it")
	-- (One from before today, the same.)
	app:set_state("board_drawers", { earlier = true })
	app:frames(30)
	takes(app, "it-old-1", "running", "resume:it-old-1", "Reopened and started", "running")
end)

test("board drop: a snoozed card on Needs you brings it back", function()
	local app = board({ snoozed = true })
	takes(app, "it-prs", "needs_you", "snooze:ak-prs:now", "Brought back", "needs_you")
end)

test("board drop: any open card on Done marks it done", function()
	local app = board(ALL)
	takes(app, "it-chat", "done", "close:it-chat:false", "Marked done", "done")
	takes(app, "it-merge", "done", "close:it-merge:false", "Marked done", "done")
	takes(app, "it-keyed", "done", "close:it-keyed:false", "Marked done", "done")
	takes(app, "it-exits", "done", "close:it-exits:false", "Marked done", "done")
	takes(app, "it-prs", "done", "close:it-prs:false", "Marked done", "done")
end)

test("board drop: Needs you takes no card", function()
	local app = board(ALL)
	refuses(app, "it-chat", "needs_you")
	refuses(app, "it-keyed", "needs_you")
	refuses(app, "it-asks", "needs_you")
end)

test("board drop: a card asking the person is answered first", function()
	local app = board(ALL)
	refuses(app, "it-merge", "running")
	refuses(app, "it-merge", "planned")
	refuses(app, "it-prs", "running")
	refuses(app, "it-prs", "planned")
end)

test("board drop: a card parked with a thread isn't started by a drop", function()
	local app = board(PLANNED)
	app:command("probes_parked")
	app:frames(10)
	refuses(app, "it-probes", "running")
end)

test("board drop: a card on its own place does nothing", function()
	local app = board(ALL)
	refuses(app, "it-chat", "running")
	refuses(app, "it-keyed", "planned")
	refuses(app, "it-asks", "done")
	refuses(app, "it-merge", "needs_you")
end)

test("board drop: one the engine refuses says why, and the card stays", function()
	local app = board(PLANNED)
	app:set_state("refuse", "That card is starting already")
	app:set_state("ds_toasts", {})
	app:frames(2)
	local before = shown(app)
	drop(app, "it-keyed", "running")
	expect(calls(app)).to_be("start:it-keyed")
	expect(shown(app) - before).to_be(1)
	local t = last(app)
	expect(t.title).to_be("Couldn't start it")
	expect(t.body).to_be("That card is starting already")
	expect(t.tone).to_be("danger")
	expect(card_of(app, "it-keyed").place).to_be("planned")
	-- (Reopen and start, refused: its own words.)
	app:set_state("ds_toasts", {})
	app:frames(2)
	drop(app, "it-asks", "running")
	t = last(app)
	expect(t.title).to_be("Couldn't do that")
	expect(t.body).to_be("That card is starting already")
	expect(card_of(app, "it-asks").place).to_be("done")
end)

test("board drop: while carried, what takes it is edged and what doesn't says why", function()
	local app = board()
	-- (Carried over nothing: Planned and Done take it; Needs you is faded.)
	app:command("carry_lifted")
	app:frames(10)
	expect(app:count(".orch-drop-lit")).to_be(2)
	expect(app:has(".orch-drop-badge")).to_be(false)
	-- Over Planned: it names what a drop does.
	app:command("carry_pause")
	app:frames(10)
	expect(app:text("#orch-drop-badge-planned")).to_be("Pause")
	-- Over Done.
	app:command("carry_done")
	app:frames(10)
	expect(app:text("#orch-drop-badge-done")).to_be("Mark done")
	-- Over Needs you: why not.
	app:command("carry_refused_needs_you")
	app:frames(10)
	expect(app:text("#orch-drop-badge-needs_you")).to_be("Only a question puts it here")
	-- A card asking the person, over Planned.
	app:command("carry_refused_answer")
	app:frames(10)
	expect(app:text("#orch-drop-badge-planned")).to_be("Answer it first")
	-- A done card over Running.
	app:command("carry_reopen_start")
	app:frames(10)
	expect(app:text("#orch-drop-badge-running")).to_be("Reopen and start")
	expect(app:error()).to_be_nil()
end)
