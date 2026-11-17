-- The board's Blocked column (ds/orch_board, ui/orch_board_columns_fixture
-- .luau): Needs you (blocked, an "asked" mark where an ask names the card)
-- and, under it, Paused (held by the person), folded until opened, its
-- count on its head; the column's head counts both. A card dropped
-- anywhere in Blocked is paused; a paused card dragged out resumes in that
-- column; Move to says Pause, and Resume for a paused card. A card the
-- engine's board check noted carries the note as one quiet line.

local function board(): any
	local app = mount("orch_board_columns_fixture.luau")
	app:resize(1440, 1000)
	app:frames(20)
	app:command("fixture.blocked_groups")
	app:frames(20)
	return app
end

-- What a node's tree line says: whether it's hidden.
local function hidden(app: any, target: string): boolean
	local first = app:tree(target):match("[^\n]+") or ""
	return first:match(" hidden%s*$") ~= nil
end

-- The cards a column shows, top to bottom (not its group heads or buttons).
local function shown_cards(app: any, col: string): { string }
	local out: { string } = {}
	for line in app:tree(`#cols-{col}`):gmatch("[^\n]+") do
		if not line:match(" hidden%s*$") then
			local t = line:match('^%s+@%d+ button "([^"]+)"')
			if
				t
				and not t:match("^Show ")
				and not t:match("^Later · ")
				and not t:match("^Paused · ")
				and t ~= "Card actions"
				and t ~= "Resume"
				and t ~= "Clear all done"
			then
				table.insert(out, t)
			end
		end
	end
	return out
end

local function has(list: { string }, title: string): boolean
	return table.find(list, title) ~= nil
end

local function card(col: string, title: string): string
	return `#cols-{col} >> role:button "{title}"`
end

-- Its menu's Move to, opened.
local function move_to(app: any, col: string, title: string)
	app:right_click(card(col, title))
	app:frames(10)
	app:click('role:menuitem "Move to"')
	app:frames(10)
end

test("orch board blocked: Needs you, then Paused folded with its count; the head counts both", function()
	local app = board()
	-- Its head: all four.
	expect(app:has('#cols-blocked >> .orch-column-count >> "4"')).to_be(true)
	expect(app:has('#cols-blocked >> role:button "Paused · 2"')).to_be(true)
	expect(app:state("columns_paused")).to_be(false)
	-- Folded: Needs you's two alone.
	local cards = shown_cards(app, "blocked")
	expect(#cards).to_be(2)
	expect(cards[1]).to_be("DNS failover between regions")
	expect(cards[2]).to_be("Wait for the cert")
	-- The one an open ask names is marked asked; the answered one isn't.
	expect(app:has(`{card("blocked", "DNS failover between regions")} >> role:img "Asked"`)).to_be(true)
	expect(app:has(`{card("blocked", "Wait for the cert")} >> role:img "Asked"`)).to_be(false)
	-- Opened: Paused's under Needs you's, each paused by you, for how long.
	app:click('#cols-blocked >> role:button "Paused · 2"')
	app:frames(20)
	expect(app:state("columns_paused")).to_be(true)
	cards = shown_cards(app, "blocked")
	expect(#cards).to_be(4)
	expect(cards[3]).to_be("Mesh admin app")
	expect(cards[4]).to_be("Moscow relay")
	local mesh = card("blocked", "Mesh admin app")
	expect(app:has(`{mesh} >> "Paused by you"`)).to_be(true)
	expect(app:has(`{mesh} >> "12m"`)).to_be(true)
	expect(app:has(`{mesh} >> .orch-card-paused`)).to_be(true)
	expect(app:has(`{mesh} >> role:img "Asked"`)).to_be(false)
	-- (An RFC3339 time, as the wire has it: an hour ago.)
	expect(app:has(`{card("blocked", "Moscow relay")} >> "Paused by you"`)).to_be(true)
	expect(app:has(`{card("blocked", "Moscow relay")} >> "1h"`)).to_be(true)
	-- Needs you's say nothing of a pause.
	expect(app:has(`{card("blocked", "Wait for the cert")} >> "Paused by you"`)).to_be(false)
	expect(app:has(`{card("blocked", "Wait for the cert")} >> .orch-card-paused`)).to_be(false)
	expect(app:error()).to_be_nil()
end)

test("orch board blocked: only paused cards: Needs you empty, Paused's head counted; none: Blocked stands aside", function()
	local app = mount("orch_board_columns_fixture.luau")
	app:resize(1440, 1000)
	app:frames(20)
	expect(hidden(app, "#cols-blocked")).to_be(true)
	app:set_state("columns_items", {
		{ id = "it-pz1", state = "blocked", priority = "normal", order = 1, title = "Mesh admin app", paused_at = 1_800_039_000 },
	})
	app:frames(20)
	expect(hidden(app, "#cols-blocked")).to_be(false)
	expect(app:has('#cols-blocked >> .orch-column-count >> "1"')).to_be(true)
	expect(app:has('#cols-blocked >> role:button "Paused · 1"')).to_be(true)
	expect(#shown_cards(app, "blocked")).to_be(0)
	expect(app:error()).to_be_nil()
end)

test("orch board blocked: a card dropped in Blocked is paused, in Needs you or Paused", function()
	local app = board()
	app:drag(card("now", "Fix slash menu in Codex chats"), card("blocked", "Wait for the cert"))
	app:frames(20)
	expect(app:state("calls")).to_contain("pause:it-now2")
	expect(app:state("calls")).never.to_contain("move:it-now2")
	-- Paused: under Paused's head (three now), not in Needs you.
	expect(app:has('#cols-blocked >> role:button "Paused · 3"')).to_be(true)
	expect(has(shown_cards(app, "blocked"), "Fix slash menu in Codex chats")).to_be(false)
	-- Onto Paused's head: paused too.
	app:drag(card("queued", "Phase 2 migration"), "#cols-blocked >> .orch-paused-head")
	app:frames(20)
	expect(app:state("calls")).to_contain("pause:it-next1")
	expect(app:has('#cols-blocked >> role:button "Paused · 4"')).to_be(true)
	-- A Needs you card dragged into Paused: paused.
	app:set_state("columns_paused", true)
	app:frames(20)
	app:drag(card("blocked", "Wait for the cert"), card("blocked", "Moscow relay"))
	app:frames(20)
	expect(app:state("calls")).to_contain("pause:it-blk2")
	expect(app:error()).to_be_nil()
end)

test("orch board blocked: within Needs you or within Paused, only its order moves", function()
	local app = board()
	app:set_state("columns_paused", true)
	app:frames(20)
	app:drag(card("blocked", "Wait for the cert"), card("blocked", "DNS failover between regions"))
	app:frames(20)
	expect(app:state("calls")).to_contain("move:it-blk2:blocked:")
	app:drag(card("blocked", "Moscow relay"), card("blocked", "Mesh admin app"))
	app:frames(20)
	expect(app:state("calls")).to_contain("move:it-pz2:blocked:")
	expect(app:state("calls")).never.to_contain("pause:")
	expect(app:state("calls")).never.to_contain("resume:")
	local cards = shown_cards(app, "blocked")
	expect(cards[1]).to_be("Wait for the cert")
	expect(cards[3]).to_be("Moscow relay")
	expect(app:error()).to_be_nil()
end)

test("orch board blocked: a paused card dragged out resumes in that column's state", function()
	local app = board()
	app:command("fixture.later")
	app:set_state("columns_paused", true)
	app:set_state("columns_later", true)
	app:frames(20)
	-- Into Now: active.
	app:drag(card("blocked", "Mesh admin app"), "#cols-now")
	app:frames(20)
	expect(app:state("calls")).to_contain("resume:it-pz1:active")
	expect(app:has(card("now", "Mesh admin app"))).to_be(true)
	-- Into Up next (onto one of its cards): planned.
	app:drag(card("blocked", "Moscow relay"), card("queued", "Phase 2 migration"))
	app:frames(20)
	expect(app:state("calls")).to_contain("resume:it-pz2:planned")
	expect(app:state("calls")).never.to_contain("move:it-pz")
	-- Paused again, then into Later (onto one of its cards): inbox.
	app:drag(card("queued", "Moscow relay"), "#cols-blocked >> .orch-paused-head")
	app:frames(20)
	app:drag(card("blocked", "Moscow relay"), card("queued", "Try a darker map"))
	app:frames(20)
	expect(app:state("calls")).to_contain("resume:it-pz2:inbox")
	-- A paused card into Needs you: it goes back (Needs you is its work's).
	app:drag(card("queued", "Try a darker map"), "#cols-blocked >> .orch-paused-head")
	app:frames(20)
	local before = app:state("calls")
	app:drag(card("blocked", "Try a darker map"), card("blocked", "DNS failover between regions"))
	app:frames(20)
	expect(app:state("calls")).to_be(before)
	expect(app:error()).to_be_nil()
end)

test("orch board blocked: Move to says Pause for a card, Resume and the rest for a paused one", function()
	local app = board()
	move_to(app, "now", "Fix slash menu in Codex chats")
	expect(app:has('role:menuitem "Pause"')).to_be(true)
	expect(app:has('role:menuitem "Blocked"')).to_be(false)
	expect(app:has('role:menuitem "Resume"')).to_be(false)
	for _, name in { "Up next", "Later", "Now", "Review", "Done today" } do
		expect(app:has(`role:menuitem "{name}"`)).to_be(true)
	end
	app:click('role:menuitem "Pause"')
	app:frames(20)
	expect(app:state("calls")).to_contain("pause:it-now2")
	expect(app:has('#cols-blocked >> role:button "Paused · 3"')).to_be(true)
	-- Paused: Resume (into Now) and the other places, no Pause or Now.
	app:set_state("columns_paused", true)
	app:frames(20)
	move_to(app, "blocked", "Mesh admin app")
	expect(app:has('role:menuitem "Resume"')).to_be(true)
	expect(app:has('role:menuitem "Pause"')).to_be(false)
	expect(app:has('role:menuitem "Blocked"')).to_be(false)
	expect(app:has('role:menuitem "Now"')).to_be(false)
	for _, name in { "Up next", "Later", "Review", "Done today" } do
		expect(app:has(`role:menuitem "{name}"`)).to_be(true)
	end
	app:click('role:menuitem "Resume"')
	app:frames(20)
	expect(app:state("calls")).to_contain("resume:it-pz1:active")
	expect(app:has(card("now", "Mesh admin app"))).to_be(true)
	-- Another place: resumed there.
	move_to(app, "blocked", "Moscow relay")
	app:click('role:menuitem "Review"')
	app:frames(20)
	expect(app:state("calls")).to_contain("resume:it-pz2:review")
	expect(app:has(card("review", "Moscow relay"))).to_be(true)
	expect(app:state("calls")).never.to_contain("move:it-pz")
	-- A Needs you card: Pause too.
	move_to(app, "blocked", "Wait for the cert")
	expect(app:has('role:menuitem "Pause"')).to_be(true)
	expect(app:has('role:menuitem "Resume"')).to_be(false)
	expect(app:error()).to_be_nil()
end)

test("orch board blocked: the engine's note is one quiet line, in the warning's ink but for a move", function()
	local app = board()
	expect(app:has(".orch-card-check")).to_be(false)
	app:command("fixture.checks")
	app:frames(20)
	local function note(col: string, title: string, words: string, tone: string)
		local c = card(col, title)
		expect(app:has(`{c} >> "{words}"`)).to_be(true)
		expect(app:has(`{c} >> .orch-check-{tone}`)).to_be(true)
		expect(app:has(`{c} >> .orch-check-{if tone == "warn" then "quiet" else "warn"}`)).to_be(false)
	end
	note("now", "Scroll position resets", "Its threads finished 20m ago", "warn")
	note("review", "Progress clocks tick by the minute", "Nothing to review", "warn")
	note("done", "Linux titlebar without controls", "A thread still works", "warn")
	note("queued", "Phase 2 migration", "Moved from Now: no thread", "quiet")
	-- One line, and only on the cards noted.
	expect(app:count(`{card("now", "Scroll position resets")} >> .orch-card-check`)).to_be(1)
	expect(app:has(`{card("now", "Fix slash menu in Codex chats")} >> .orch-card-check`)).to_be(false)
	expect(app:error()).to_be_nil()
end)

test("orch board blocked: a paused card has a Resume of its own, into Now; Needs you's have none", function()
	local app = board()
	app:click('#cols-blocked >> role:button "Paused · 2"')
	app:frames(20)
	local paused = card("blocked", "Mesh admin app")
	expect(app:has(`{paused} >> .orch-card-resume`)).to_be(true)
	expect(hidden(app, `{paused} >> .orch-card-resume`)).to_be(false)
	expect(app:has(`{card("blocked", "Wait for the cert")} >> .orch-card-resume`)).to_be(false)
	app:click(`{paused} >> .orch-card-resume`)
	app:frames(20)
	expect(app:state("calls")).to_contain("resume:it-pz1:active")
	expect(has(shown_cards(app, "now"), "Mesh admin app")).to_be(true)
end)

test("orch board blocked: a Resume or Pause refused puts the card back at once, and says so", function()
	local app = board()
	app:set_state("columns_refuse", true)
	app:click('#cols-blocked >> role:button "Paused · 2"')
	app:frames(20)
	app:click(`{card("blocked", "Mesh admin app")} >> .orch-card-resume`)
	app:frames(20)
	expect(app:state("calls")).to_contain("resume:it-pz1:active")
	expect(app:has(`~"Couldn't resume it"`)).to_be(true)
	-- (Back in Paused, not held in Now till the engine speaks.)
	expect(has(shown_cards(app, "now"), "Mesh admin app")).to_be(false)
	expect(has(shown_cards(app, "blocked"), "Mesh admin app")).to_be(true)
	move_to(app, "now", "Scroll position resets")
	app:click('role:menuitem "Pause"')
	app:frames(20)
	expect(app:state("calls")).to_contain("pause:it-now1")
	expect(app:has(`~"Couldn't pause it"`)).to_be(true)
	expect(has(shown_cards(app, "now"), "Scroll position resets")).to_be(true)
end)
