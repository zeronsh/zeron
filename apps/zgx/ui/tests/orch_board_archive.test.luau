-- Archive all done (ds/orch_board_actions `archive`): every closed card
-- (Done's and Earlier's) off the board at once, each sent by the host's
-- delayed delete once the Undo's while is over (one removeItem per card),
-- one Undo putting them all back untouched; refused, back with a toast in
-- the danger's tone. From Done's head on the board and in the list
-- (ui/orch_board_places_fixture.luau: three Done, five Earlier), on an
-- orchestrator's own list, and from the Map's menus, which also put the
-- cards' threads away from the Map (ui/orch_map_fixture.luau).

local CLOSED = { "it-asks", "it-token", "it-msi", "it-old-1", "it-old-2", "it-old-3", "it-old-4", "it-old-5" }

local function board(view: string?): any
	local app = mount("orch_board_places_fixture.luau")
	app:resize(1700, 1200)
	app:set_state("board_drawers", { earlier = true })
	if view then
		app:set_state("board_view", view)
	end
	app:frames(30)
	expect(app:error()).to_be_nil()
	return app
end

local function calls(app: any): string
	return app:state("calls") or ""
end

local function rows(app: any): string
	local out = {}
	for _, list in { "fx_items", "fx_cards" } do
		for _, r in app:state(list) or {} do
			local keys = {}
			for k in r do
				table.insert(keys, k)
			end
			table.sort(keys)
			for _, k in keys do
				local v = r[k]
				table.insert(out, `{r.id}.{k}={if type(v) == "table" then `#{#v}` else tostring(v)}`)
			end
		end
	end
	return table.concat(out, ",")
end

local function newest(app: any): any
	local got = nil
	for _, t in app:state("ds_toasts") or {} do
		if got == nil or t.id > got.id then
			got = t
		end
	end
	return got
end

local function shown_on_board(app: any, id: string): boolean
	return app:has(`#orch-card-{id}`) or app:has(`#orch-row-{id}`)
end

-- Each id noted once with `what` (`remove`, `sent`, `refused`, `undo`).
local function each_once(app: any, what: string)
	local seen: { [string]: number } = {}
	for part in string.gmatch(calls(app), "[^,]+") do
		seen[part] = (seen[part] or 0) + 1
	end
	for _, id in CLOSED do
		local n = seen[`{what}:{id}`] or 0
		if n ~= 1 then
			expect(`{what}:{id} x{n} in {calls(app)}`).to_be(`{what}:{id} once`)
		end
	end
end

test("board actions: archive all done, from Done's head, sends each closed card's removal once, after the Undo's while", function()
	local app = board()
	expect(app:has("#orch-col-done >> .orch-archive-done")).to_be(true)
	app:click("#orch-col-done >> .orch-archive-done")
	app:frames(10)
	-- Off the board at once, nothing sent yet.
	for _, id in CLOSED do
		expect(shown_on_board(app, id)).to_be(false)
	end
	each_once(app, "remove")
	expect(string.find(calls(app), "sent:", 1, true)).to_be_nil()
	-- Open cards untouched.
	expect(app:has("#orch-card-it-chat")).to_be(true)
	local t = newest(app)
	expect(t.title).to_be("Archived 8 done tasks")
	expect(t.action.label).to_be("Undo")
	-- Its button gone with them (nothing closed left).
	expect(app:has("#orch-col-done >> .orch-archive-done")).to_be(false)
	app:wait('"never shown, only waited past"', { timeout_ms = 7000 })
	each_once(app, "sent")
	app:wait('"never shown, only waited past"', { timeout_ms = 4500 })
	for _, id in CLOSED do
		expect(shown_on_board(app, id)).to_be(false)
	end
	expect(app:error()).to_be_nil()
end)

test("board actions: archive all done's Undo puts every card back exactly as it was, nothing sent", function()
	local app = board()
	local was = rows(app)
	app:click("#orch-col-done >> .orch-archive-done")
	app:frames(10)
	app:click('role:button "Undo"')
	app:frames(20)
	for _, id in CLOSED do
		expect(shown_on_board(app, id)).to_be(true)
	end
	app:wait('"never shown, only waited past"', { timeout_ms = 7000 })
	each_once(app, "undo")
	expect(string.find(calls(app), "sent:", 1, true)).to_be_nil()
	expect(rows(app)).to_be(was)
	for _, id in CLOSED do
		expect(shown_on_board(app, id)).to_be(true)
	end
end)

test("board actions: archive all done the engine refuses brings the cards back, said once in the danger's tone", function()
	local app = board()
	local was = rows(app)
	app:click("#orch-col-done >> .orch-archive-done")
	app:frames(10)
	-- (The engine refuses each once its wait is over: Orchestrators turned
	-- off meanwhile is refused so by the host.)
	app:set_state("refuse", "Orchestrators is off")
	app:set_state("ds_toasts", {})
	app:wait('"never shown, only waited past"', { timeout_ms = 7000 })
	app:frames(10)
	each_once(app, "refused")
	for _, id in CLOSED do
		expect(shown_on_board(app, id)).to_be(true)
	end
	expect(rows(app)).to_be(was)
	local danger = 0
	for _, t in app:state("ds_toasts") or {} do
		if t.tone == "danger" then
			danger += 1
			expect(t.title).to_be("Couldn't archive them")
			expect(t.body).to_be("Orchestrators is off")
		end
	end
	expect(danger).to_be(1)
end)

test("board actions: archive all done refused as it's asked (the host off) leaves every card where it was", function()
	local app = board()
	local was = rows(app)
	app:set_state("refuse", "Orchestrators is off")
	app:click("#orch-col-done >> .orch-archive-done")
	app:frames(20)
	for _, id in CLOSED do
		expect(shown_on_board(app, id)).to_be(true)
	end
	expect(rows(app)).to_be(was)
	-- (One toast, in the danger's tone; no Archived, no Undo.)
	expect(newest(app).tone).to_be("danger")
	expect(newest(app).title).to_be("Couldn't archive them")
	expect(string.find(calls(app), "sent:", 1, true)).to_be_nil()
end)

test("board actions: archive all done from the list's Done section, and on an orchestrator's own list", function()
	local app = board("list")
	expect(app:has("#orch-list-done >> .orch-archive-done")).to_be(true)
	app:click("#orch-list-done >> .orch-archive-done")
	app:frames(10)
	expect(app:has("#orch-list-done >> .orch-list-row")).to_be(false)
	each_once(app, "remove")
	-- Undone: back in the section, still open (its head's button folds
	-- nothing).
	app:click('role:button "Undo"')
	app:frames(20)
	expect(app:has("#orch-list-done >> #orch-row-it-asks")).to_be(true)
	-- One orchestrator's list (a page's side panel): its closed cards alone.
	local one = mount("orch_board_places_fixture.luau")
	one:resize(900, 1200)
	one:set_state("only_list", true)
	one:set_state("compact", true)
	one:set_state("filter", "orc-ci")
	one:frames(30)
	expect(one:has("#fixture-list >> .orch-archive-done")).to_be(true)
	one:click("#fixture-list >> .orch-archive-done")
	one:frames(10)
	expect(calls(one)).to_be("remove:it-msi")
end)

test("board actions: no archive where the host deletes no cards", function()
	local app = mount("orch_board_list_fixture.luau")
	app:resize(1500, 1000)
	app:frames(30)
	expect(app:has(".orch-archive-done")).to_be(false)
end)

-- The Map (the Control room page's own action, ui/orch_map_fixture.luau).
local function map(): any
	local app = mount("orch_map_fixture.luau")
	app:resize(1440, 2400)
	app:command("map_only")
	app:command("map_force")
	app:set_state("ds/orch_map_visibility.luau::control_map_show", "recent")
	app:frames(60)
	return app
end
local function on_map(app: any, text: string): boolean
	return string.find(app:tree(".orch-map") :: string, text, 1, true) ~= nil
end

test("board actions: archive all done from the Map's space menu takes its cards' threads off the Map; Undo brings them back", function()
	local app = map()
	app:command("map_archive_count")
	app:frames(1)
	local a = app:state("archive")
	expect(a.count).to_be(3)
	-- (Its threads on the Map before: finished, unread.)
	expect(on_map(app, "Code signing")).to_be(true)
	expect(on_map(app, "Moscow node reboot plan")).to_be(true)
	expect(on_map(app, "README usage")).to_be(true)
	app:right_click(".orch-force-legend")
	app:frames(5)
	expect(app:has('role:menuitem "Archive all done"')).to_be(true)
	app:click('role:menuitem "Archive all done"')
	app:frames(30)
	expect(on_map(app, "README usage")).to_be(false)
	expect(on_map(app, "Code signing")).to_be(false)
	-- (A card whose item still says Review, from an engine before places,
	-- keeps its thread on the Map, as the Map's rule keeps every chat on a
	-- card in Review, until its removal lands.)
	expect(on_map(app, "Moscow node reboot plan")).to_be(true)
	expect(string.find(tostring(app:state("calls")), "removeItem", 1, true)).to_be_nil()
	app:command("map_archive_undo")
	app:frames(30)
	expect(on_map(app, "Code signing")).to_be(true)
	expect(on_map(app, "README usage")).to_be(true)
	app:wait(6500)
	app:frames(5)
	expect(string.find(tostring(app:state("calls")), "removeItem", 1, true)).to_be_nil()
	-- Again, no Undo: each card's removal sent once, its threads still off.
	app:command("map_archive_do")
	app:frames(10)
	app:wait(6500)
	app:frames(10)
	local c = tostring(app:state("calls"))
	for _, id in a.ids do
		local n = 0
		for part in string.gmatch(c, "[^,]+") do
			if part == `op:removeItem:{id}` then
				n += 1
			end
		end
		expect(n).to_be(1)
	end
	expect(on_map(app, "Code signing")).to_be(false)
	expect(on_map(app, "README usage")).to_be(false)
end)

test("board actions: the Map's own menu offers archive all done too", function()
	local app = map()
	app:command("map_only")
	app:frames(10)
	-- (The Map block's menu: its views, what it shows, and Archive all done.)
	app:click(".control-panel-menu >> role:button")
	app:frames(10)
	expect(app:has('role:menuitem "Archive all done"')).to_be(true)
end)

test("board actions: archive all done on an orchestrator's page (its Tasks list), its own closed cards alone", function()
	local app = mount("orchestrator_fixture.luau")
	app:resize(1440, 900)
	app:frames(30)
	-- (Nothing closed of its yet: no button.)
	expect(app:has("#orch-tasks >> .orch-archive-done")).to_be(false)
	-- One of its cards closed today, and one of another orchestrator's.
	local now = table.clone(app:state("orchestration"))
	local items = table.clone(now.items or {})
	local cards = table.clone(now.cards or {})
	local t = os.time()
	for _, c in { { "it-fx-mine", "orc-orch" }, { "it-fx-theirs", "orc-vpn" } } do
		table.insert(items, { id = c[1], orchestrator_id = c[2], title = `Closed {c[1]}`, state = "done", priority = "normal", threads = {}, links = {}, order = 99, created = t - 600, updated = t - 60, done = t - 60 })
		table.insert(cards, { id = c[1], place = "done", status = "done", old_state = "done", since = t - 60, status_since = t - 60, asks = {}, closed = { at = t - 60, by = "engine", how = "report" }, finish = { line = "report" }, events = {} })
	end
	now.items = items
	now.cards = cards
	app:set_state("orchestration", now)
	app:frames(30)
	expect(app:has("#orch-tasks >> .orch-archive-done")).to_be(true)
	app:click("#orch-tasks >> .orch-archive-done")
	app:frames(10)
	expect(app:has("#orch-tasks >> #orch-row-it-fx-mine")).to_be(false)
	app:wait(6500)
	app:frames(10)
	local c = tostring(app:state("orch_calls"))
	expect(string.find(c, "it-fx-mine", 1, true) ~= nil).to_be(true)
	expect(string.find(c, "removeItem", 1, true) ~= nil).to_be(true)
	expect(string.find(c, "it-fx-theirs", 1, true)).to_be_nil()
end)

-- The board's own menu: its "…" in its head, and a secondary click around
-- its cards (a column's head, the list around its rows), on the Control
-- room's board and an orchestrator's page: Archive all done, Unpin all and
-- Mark all done as read (the Map's own). A card's secondary click is its
-- own menu still.
local function control_board(): any
	local app = mount("control_needs_cards_fixture.luau")
	app:resize(1600, 1000)
	app:frames(30)
	app:command("fx.board")
	app:frames(30)
	return app
end
local THREE = { "Archive all done", "Unpin all" }
local function has_three(app: any): boolean
	for _, l in THREE do
		if not app:has(`role:menuitem "{l}"`) then
			return false
		end
	end
	return app:has('role:menuitem ~"done as read"')
end

test("board actions: the board's … menu has Archive all done, Unpin all and Mark all done as read", function()
	local app = control_board()
	expect(app:has("#orch-board-more")).to_be(true)
	app:click("#orch-board-more")
	app:frames(10)
	expect(has_three(app)).to_be(true)
	app:key("Escape")
	app:frames(5)
	-- Unpin all lets go of the Map's kept pins, the Map not built.
	app:set_state("ds/orch_force.luau::control_map_force_pins", { ["th-ui-orch"] = true })
	app:click("#orch-board-more")
	app:frames(10)
	app:click('role:menuitem "Unpin all"')
	app:frames(10)
	expect(next(app:state("ds/orch_force.luau::control_map_force_pins") or {})).to_be_nil()
	expect(app:error()).to_be_nil()
end)

test("board actions: a secondary click around the cards opens the board's menu; on a card, the card's own", function()
	local app = control_board()
	app:right_click("#orch-col-running >> .orch-column-head")
	app:frames(10)
	expect(has_three(app)).to_be(true)
	app:key("Escape")
	app:frames(5)
	local card = string.match(app:tree("#orch-col-running", 3), "#(orch%-card%-[%w%-]+)")
	expect(card ~= nil).to_be(true)
	app:right_click(`#{card}`)
	app:frames(10)
	expect(app:has('role:menu "Card actions" >> role:menuitem "Unpin all"')).to_be(false)
	expect(app:has('role:menu "Card actions" >> role:menuitem "Delete from the board"')).to_be(true)
	app:key("Escape")
	app:frames(5)
	-- In the list too.
	app:click("#orch-board-view-list")
	app:frames(40)
	app:right_click("#orch-list-running >> .orch-fold-head")
	app:frames(10)
	-- (A section's head folds on a click; its secondary click is the
	-- board's.)
	expect(has_three(app)).to_be(true)
end)

test("board actions: an orchestrator's page list has the board's menu too", function()
	local app = mount("orchestrator_fixture.luau")
	app:resize(1440, 900)
	app:frames(30)
	expect(app:has("#orch-tasks >> #orch-list-more")).to_be(true)
	app:click("#orch-tasks >> #orch-list-more")
	app:frames(10)
	expect(has_three(app)).to_be(true)
	app:key("Escape")
	app:frames(5)
	app:right_click("#orch-tasks >> #orch-list-needs_you >> .orch-fold-head")
	app:frames(10)
	expect(has_three(app)).to_be(true)
end)
