-- An orchestrator's side panel (screens/orchestrator/panel) on its page, in
-- the sample world (ui/orchestrator_fixture.luau): its Tasks and Crew tabs
-- and their counts; folding sections kept for each orchestrator; the crew
-- grouped, capped and said in plain words; the panel sized by its seam
-- within bounds; Chat, Split and Live at every width; a card or a crew
-- thread opened in the panel, Open thread leaving for it; a new
-- orchestrator starting on its conversation alone; and no symbol typed as
-- text anywhere in it.

local function settle(app: any)
	app:frames(30)
end

local function open(w: number?, h: number?): any
	local app = mount("orchestrator_fixture.luau")
	app:resize(w or 1440, h or 900)
	settle(app)
	return app
end

-- Whether a node shows (there, and not hidden, nor anything over it).
local function shown(app: any, sel: string): boolean
	if not app:has(sel) then
		return false
	end
	local first = string.match(app:tree(sel), "[^\n]+") or ""
	if string.find(first, " hidden", 1, true) then
		return false
	end
	local box = string.match(first, " %-?[%d%.]+,%-?[%d%.]+ ([%d%.]+)x[%d%.]+")
	return box == nil or tonumber(box) :: number > 0
end

local function box(app: any, sel: string): { x: number, y: number, w: number, h: number }
	local first = string.match(app:tree(sel, 0), "[^\n]+") or ""
	local x, y, w, h = string.match(first, " (%-?[%d%.]+),(%-?[%d%.]+) ([%d%.]+)x([%d%.]+)")
	return { x = tonumber(x) or 0, y = tonumber(y) or 0, w = tonumber(w) or 0, h = tonumber(h) or 0 }
end

local function crew_tab(app: any)
	app:click("#orch-panel-tabs >> role:tab [2]")
	settle(app)
end

local function kept(app: any, id: string): any
	local all = app:state("orch_panel") :: any
	return (all and all[id]) or {}
end

test("side panel: Tasks and Crew tabs, each with its count", function()
	local app = open()
	expect(app:has('#orch-panel-tabs >> role:tab "Tasks"')).to_be(true)
	expect(app:has('#orch-panel-tabs >> role:tab "Crew"')).to_be(true)
	-- Its five tasks not done, its four crew threads.
	expect(app:has('#orch-panel-tabs >> role:tab "Tasks" >> "5"')).to_be(true)
	expect(app:has('#orch-panel-tabs >> role:tab "Crew" >> "4"')).to_be(true)
	expect(shown(app, "#orch-panel-tasks")).to_be(true)
	expect(shown(app, "#orch-panel-crew")).to_be(false)
	crew_tab(app)
	expect(shown(app, "#orch-panel-crew")).to_be(true)
	expect(shown(app, "#orch-panel-tasks")).to_be(false)
	expect(kept(app, "orc-orch").tab).to_be("crew")
	expect(app:error()).to_be_nil()
end)

test("side panel: the header says how many work, of how many", function()
	local app = open()
	expect(app:has('#orch-crew-count >> "3 of 4 working"')).to_be(true)
end)

-- (The Tasks tab's list, ds/orch_board_list: its Needs you head, and the
-- row under it.)
local NEEDS_HEAD = '#orch-list-needs_you >> role:button "Needs you"'
local NEEDS_ROW = "#orch-row-it-orch-screen"

test("side panel: sections fold, and stay folded for that orchestrator", function()
	local app = open()
	expect(shown(app, NEEDS_ROW)).to_be(true)
	app:click(NEEDS_HEAD)
	settle(app)
	expect(shown(app, NEEDS_ROW)).to_be(false)
	expect(kept(app, "orc-orch").folded.needs_you).to_be(true)
	-- Another orchestrator's own folds; back, still folded, and on its tab.
	crew_tab(app)
	-- (As a person switches: the sidebar's row clicked, focus off the tabs.)
	app:click('#sidebar-orchestration >> role:button "Exnomic VPN"')
	settle(app)
	expect(app:state("orch_route")).to_be("orchestrator:orc-vpn")
	expect(shown(app, "#orch-panel-tasks")).to_be(true)
	expect(shown(app, "#orch-row-it-moscow")).to_be(true)
	app:click('#sidebar-orchestration >> role:button "Orchestrators" [2]')
	settle(app)
	expect(app:state("orch_route")).to_be("orchestrator:orc-orch")
	expect(shown(app, "#orch-panel-crew")).to_be(true)
	app:click("#orch-panel-tabs >> role:tab [1]")
	settle(app)
	expect(shown(app, NEEDS_ROW)).to_be(false)
	app:click(NEEDS_HEAD)
	settle(app)
	expect(shown(app, NEEDS_ROW)).to_be(true)
	expect(kept(app, "orc-orch").folded.needs_you).to_be(false)
end)

test("side panel: the crew grouped Working, Waiting on you, Idle; each capped", function()
	local app = open()
	app:command("orch.big_crew")
	settle(app)
	crew_tab(app)
	-- 8 of its own working and 3 of the sample's; 5 shown, then "6 more".
	expect(app:count("#orch-crew-working >> .orch-crew-row")).to_be(5)
	expect(app:has('#orch-crew-working >> role:button "6 more"')).to_be(true)
	app:click('#orch-crew-working >> role:button "6 more"')
	settle(app)
	expect(app:count("#orch-crew-working >> .orch-crew-row")).to_be(11)
	-- Waiting on you: the three failed and the sample's thread with an ask.
	expect(app:has('#orch-crew-waiting >> "Broken thread 1"')).to_be(true)
	expect(app:has('#orch-crew-waiting >> "Orchestrator screen"')).to_be(true)
	-- Idle folded: its count and its latest thread, no rows.
	expect(app:count("#orch-crew-idle >> .orch-crew-row")).to_be(0)
	expect(app:has('#orch-crew-idle >> .orch-crew-latest >> "Quiet thread 1"')).to_be(true)
	expect(app:text("#orch-crew-idle >> .orch-fold-count")).to_be("7")
	app:scroll("#orch-crew-scroll", 2000)
	settle(app)
	app:click("#orch-crew-idle >> .orch-fold-head")
	settle(app)
	expect(app:count("#orch-crew-idle >> .orch-crew-row")).to_be(5)
	expect(kept(app, "orc-orch").folded.crew_idle).to_be(false)
	-- Each thread's name once; plain status words, never a playful verb.
	local all = app:tree("#orch-crew-list")
	local _, names = string.gsub(all, '"Busy thread 1"', "")
	expect(names).to_be(1)
	expect(app:has("#orch-crew-list >> .orch-working")).to_be(false)
	expect(app:has('#orch-crew-list >> "Failed"')).to_be(true)
	expect(app:error()).to_be_nil()
end)

-- (The seam's drag writes the width it asks for into what's kept, and the
-- page holds it within bounds; the test host moves no pointer, so the
-- width is asked for as the drag would ask.)
local function want_width(app: any, w: number)
	local all = table.clone((app:state("orch_panel") :: any) or {})
	local one = table.clone(all["orc-orch"] or {})
	one.w = w
	all["orc-orch"] = one
	app:set_state("orch_panel", all)
	settle(app)
end

test("side panel: its width as dragged, within its bounds", function()
	local app = open(1440, 900)
	expect(app:has("#orch-seam-handle")).to_be(true)
	local page = box(app, "#orch-body")
	local before = box(app, "#orch-side").w
	want_width(app, 520)
	expect(math.abs(box(app, "#orch-side").w - 520) < 1.5).to_be(true)
	expect(box(app, "#orch-side").w ~= before).to_be(true)
	-- Far too wide: as wide as leaves the conversation its least.
	want_width(app, 5000)
	expect(box(app, "#orch-conversation").w >= 360 - 1).to_be(true)
	expect(box(app, "#orch-side").w <= page.w - 360).to_be(true)
	-- Far too narrow: its least.
	want_width(app, 40)
	expect(math.abs(box(app, "#orch-side").w - 300) < 1.5).to_be(true)
	-- Kept as asked; a narrower window holds it within bounds again.
	want_width(app, 700)
	app:resize(1200, 900)
	settle(app)
	expect(box(app, "#orch-conversation").w >= 360 - 1).to_be(true)
	expect(box(app, "#orch-side").w >= 300 - 1).to_be(true)
end)

test("side panel: Chat, Split and Live, wide", function()
	local app = open(1440, 900)
	expect(app:has('#orch-mode >> role:radio "Split"')).to_be(true)
	expect(shown(app, "#orch-conversation")).to_be(true)
	expect(shown(app, "#orch-side")).to_be(true)
	app:click('#orch-mode >> role:radio "Chat"')
	settle(app)
	expect(shown(app, "#orch-side")).to_be(false)
	expect(shown(app, "#orch-conversation")).to_be(true)
	expect(kept(app, "orc-orch").mode).to_be("chat")
	app:click('#orch-mode >> role:radio "Live"')
	settle(app)
	expect(shown(app, "#orch-side")).to_be(true)
	expect(shown(app, "#orch-conversation")).to_be(false)
	-- Kept for it: another orchestrator splits, this one comes back Live.
	app:command("orch.vpn")
	settle(app)
	expect(shown(app, "#orch-conversation")).to_be(true)
	expect(shown(app, "#orch-side")).to_be(true)
	app:command("orch.orch")
	settle(app)
	expect(shown(app, "#orch-conversation")).to_be(false)
	expect(shown(app, "#orch-side")).to_be(true)
end)

test("side panel: Chat, Split and Live, narrow", function()
	local app = open(900, 800)
	-- The switch shows narrow too; Split with no room for both, the chat.
	expect(app:has('#orch-mode >> role:radio "Split"')).to_be(true)
	expect(shown(app, "#orch-conversation")).to_be(true)
	expect(shown(app, "#orch-side")).to_be(false)
	app:click('#orch-mode >> role:radio "Live"')
	settle(app)
	expect(shown(app, "#orch-side")).to_be(true)
	expect(shown(app, "#orch-conversation")).to_be(false)
	app:click('#orch-mode >> role:radio "Chat"')
	settle(app)
	expect(shown(app, "#orch-side")).to_be(false)
	-- Widened, Split again shows both.
	app:click('#orch-mode >> role:radio "Split"')
	app:resize(1600, 900)
	settle(app)
	expect(shown(app, "#orch-conversation")).to_be(true)
	expect(shown(app, "#orch-side")).to_be(true)
end)

test("side panel: a crew thread opens in the panel; Open thread leaves for it", function()
	local app = open()
	crew_tab(app)
	app:click('#orch-crew-list >> role:button "Open Engine: orchestration state"')
	settle(app)
	expect(app:state("orch_route")).to_be("orchestrator:orc-orch")
	expect(shown(app, "#orch-detail")).to_be(true)
	expect(shown(app, "#orch-panel-tabs")).to_be(false)
	expect(app:text("#orch-thread-title")).to_be("Engine: orchestration state")
	expect(app:has("#orch-thread-status")).to_be(true)
	expect(app:has("#orch-thread-machine")).to_be(true)
	expect(app:has("#orch-thread-model")).to_be(true)
	-- Back: the tabs, as they were.
	app:click("#orch-thread-back")
	settle(app)
	expect(shown(app, "#orch-detail")).to_be(false)
	expect(shown(app, "#orch-panel-crew")).to_be(true)
	app:click('#orch-crew-list >> role:button "Open Engine: orchestration state"')
	settle(app)
	app:click("#orch-thread-open")
	settle(app)
	expect(app:state("orch_route")).to_be("session:th-engine-state")
end)

test("side panel: a card opens in the panel as the board's detail, its ask with its answers", function()
	local app = open()
	app:click("#orch-row-it-orch-screen")
	settle(app)
	expect(app:state("orch_route")).to_be("orchestrator:orc-orch")
	expect(shown(app, "#orch-detail")).to_be(true)
	expect(shown(app, "#orch-panel-tabs")).to_be(false)
	-- The board's own detail (ds/orch_board_detail), not a second one.
	expect(app:has("#orch-detail >> .orch-detail")).to_be(true)
	expect(app:has("#orch-thread-status")).to_be(false)
	expect(app:has('#orch-detail >> "Orchestrator screen"')).to_be(true)
	expect(app:has('#orch-detail >> role:button "Side by side"')).to_be(true)
	-- Back: the tabs again, nothing asked of the engine.
	local before = app:state("orch_calls")
	app:click('#orch-detail >> role:button "Back"')
	settle(app)
	expect(shown(app, "#orch-detail")).to_be(false)
	expect(shown(app, "#orch-panel-tabs")).to_be(true)
	expect(app:state("orch_calls")).to_be(before)
end)

test("side panel: a card's Back leaves it as it is; Mark done in its detail closes it", function()
	local app = open()
	app:click("#orch-row-it-orch-screen")
	settle(app)
	local before = app:state("orch_calls")
	app:click('#orch-detail >> role:button "Back"')
	settle(app)
	expect(app:state("orch_calls")).to_be(before)
	app:click("#orch-row-it-orch-screen")
	settle(app)
	app:click("#orch-detail-menu")
	app:frames(10)
	app:click('role:menuitem "Mark done"')
	settle(app)
	local after: string = app:state("orch_calls") or ""
	expect(after).to_contain("closeItem:")
	expect(after).to_contain("it-orch-screen")
	-- (Marked done: the detail stays on the card, not sent back.)
	expect(shown(app, "#orch-detail")).to_be(true)
end)

test("side panel: a new orchestrator shows the full layout, its panel saying what's empty", function()
	local app = open()
	app:command("orch.fresh")
	settle(app)
	expect(app:has('#orch-name >> "Release train"')).to_be(true)
	-- Split from the start: the conversation, the panel, the switch.
	expect(shown(app, "#orch-conversation")).to_be(true)
	expect(shown(app, "#orch-side")).to_be(true)
	expect(shown(app, "#orch-mode")).to_be(true)
	expect(app:has('#orch-mode >> role:radio "Split"')).to_be(true)
	-- Its empty chat says what to type first.
	expect(shown(app, "#orch-first-hint")).to_be(true)
	-- Both tabs, their counts 0 (a count of 0 is not drawn).
	expect(app:has('#orch-panel-tabs >> role:tab "Tasks"')).to_be(true)
	expect(app:has('#orch-panel-tabs >> role:tab "Crew"')).to_be(true)
	expect(app:has('#orch-panel-tabs >> role:tab "Tasks" >> "1"')).to_be(false)
	expect(app:has('#orch-panel-tabs >> role:tab "Crew" >> "1"')).to_be(false)
	-- Tasks: the list's own empty sections.
	expect(app:wait('#orch-list-needs_you >> "Nothing needs you"', { timeout_ms = 2000 })).to_be(true)
	expect(app:has('#orch-list-running >> "Nothing is running"')).to_be(true)
	expect(app:has('#orch-list-done >> "Nothing finished today"')).to_be(true)
	-- Crew: no crew yet.
	crew_tab(app)
	expect(shown(app, "#orch-crew-list >> .orch-crew-empty")).to_be(true)
	expect(app:has('#orch-crew-list >> "No crew yet"')).to_be(true)
	-- Its first task: a row in the list.
	app:click("#orch-panel-tabs >> role:tab [1]")
	app:command("orch.fresh_task")
	settle(app)
	expect(shown(app, "#orch-row-it-fresh")).to_be(true)
	expect(app:has('#orch-row-it-fresh >> "Release notes"') or app:has('#orch-row-it-fresh "Release notes"')).to_be(true)
	expect(app:error()).to_be_nil()
end)

test("side panel: a mode chosen for a new orchestrator still wins", function()
	local app = open()
	app:command("orch.fresh")
	settle(app)
	app:click('#orch-mode >> role:radio "Chat"')
	settle(app)
	expect(shown(app, "#orch-side")).to_be(false)
	app:click('#sidebar-orchestration >> role:button "Exnomic VPN"')
	settle(app)
	expect(shown(app, "#orch-side")).to_be(true)
	app:click('#sidebar-orchestration >> role:button "Release train"')
	settle(app)
	expect(app:state("orch_route")).to_be("orchestrator:orc-fresh")
	expect(shown(app, "#orch-side")).to_be(false)
	expect(shown(app, "#orch-conversation")).to_be(true)
end)

test("side panel: the pause and settings say what they do; paused shows", function()
	local app = open()
	expect(app:has('#orch-pause')).to_be(true)
	expect(app:has('#orch-paused')).to_be(false)
	app:command("orch.fresh")
	settle(app)
	app:command("orch.orch")
	settle(app)
	local now = table.clone(app:state("orchestration") :: any)
	local list = {}
	for _, o in now.orchestrators do
		if o.id == "orc-orch" then
			o = table.clone(o)
			o.paused = true
		end
		table.insert(list, o)
	end
	now.orchestrators = list
	app:set_state("orchestration", now)
	settle(app)
	expect(app:has('#orch-paused >> "Paused"')).to_be(true)
	expect(app:has('#orch-pause >> "Resume"')).to_be(true)
end)

-- Symbols typed as text, never in the UI: icons and spacing instead.
local SYMBOLS = { "·", "•", "→", "←", "✓", "✔", "✗", "✕", "×", "▸", "▾", "›", "»" }
local function no_symbols(app: any, sel: string)
	local t = app:tree(sel)
	for _, s in SYMBOLS do
		if string.find(t, s, 1, true) then
			-- (Said with what it was found in.)
			local line = ""
			for l in t:gmatch("[^\n]+") do
				if string.find(l, s, 1, true) then
					line = l
					break
				end
			end
			expect(`{sel}: {line}`).to_be(`{sel}: no {s}`)
		end
	end
end

test("side panel: no symbol typed as text, in its head, tabs, crew or a detail", function()
	local app = open()
	app:command("orch.big_crew")
	settle(app)
	no_symbols(app, "#orch-head")
	no_symbols(app, "#orch-panel-tabs")
	no_symbols(app, NEEDS_HEAD)
	no_symbols(app, '#orch-list-running >> role:button "Running"')
	crew_tab(app)
	app:scroll("#orch-crew-scroll", 2000)
	settle(app)
	app:click("#orch-crew-idle >> .orch-fold-head")
	settle(app)
	no_symbols(app, "#orch-crew-list")
	app:click('#orch-crew-list >> role:button "Open Orchestrator screen"')
	settle(app)
	no_symbols(app, "#orch-detail")
end)

-- A thread done with an ask of its still open: waiting on you, since it
-- asked, in its Crew row and its detail; "Completed" only once nothing is.
local function thread_status(app: any, chat: string, st: string)
	local s = table.clone(app:state("sessions") :: any)
	local kids = table.clone(s.children or {})
	local mine = {}
	for _, c in kids["orc-orch"] or {} do
		if c.id == chat then
			c = table.clone(c)
			c.status = st
		end
		table.insert(mine, c)
	end
	kids["orc-orch"] = mine
	s.children = kids
	app:set_state("sessions", s)
	settle(app)
end

test("side panel: a finished thread with an open ask reads Waiting on you, not Completed", function()
	local app = open()
	thread_status(app, "th-ui-orch", "done")
	crew_tab(app)
	local ROW = '#orch-crew-waiting >> role:button "Open Orchestrator screen"'
	expect(app:has(`{ROW} >> .orch-status-input`)).to_be(true)
	expect(app:has(`{ROW} >> .orch-status-done`)).to_be(false)
	-- Its time is since it asked (the sample's ask: 200 s, "3m").
	expect(app:has(`{ROW} >> "3m"`)).to_be(true)
	app:click(ROW)
	settle(app)
	expect(app:has('#orch-thread-status >> "Waiting on you"')).to_be(true)
	expect(app:has('#orch-thread-status >> "Completed"')).to_be(false)
	expect(app:has("#orch-thread-status >> .orch-status-input")).to_be(true)
	expect(app:has('#orch-thread-status >> "3m"')).to_be(true)
	-- Answered: nothing open, the thread done.
	local now = table.clone(app:state("orchestration") :: any)
	local asks = {}
	for _, a in now.asks do
		if a.id == "ak-1" then
			a = table.clone(a)
			a.state = "answered"
		end
		table.insert(asks, a)
	end
	now.asks = asks
	app:set_state("orchestration", now)
	settle(app)
	expect(app:has('#orch-thread-status >> "Completed"')).to_be(true)
	expect(app:has('#orch-thread-status >> "Waiting on you"')).to_be(false)
	expect(app:has("#orch-thread-status >> .orch-status-done")).to_be(true)
	app:click("#orch-thread-back")
	settle(app)
	expect(app:has('#orch-crew-idle >> .orch-crew-latest >> "Orchestrator screen"')).to_be(true)
end)

-- The seam's drag (ds/orch_seam_drag), the page's handler itself, driven
-- with pointer positions: the width it keeps after each move.
test("side panel: the seam's drag keeps the width each position asks, within bounds", function()
	local SeamDrag = require("@ds/orch_seam_drag")
	local kept: number? = 480
	local page = 1200
	local seen: { { number } } = {}
	local drag = SeamDrag.new {
		width = function(): number
			return kept or 0
		end,
		page = function(): number
			return page
		end,
		set = function(w: number?)
			kept = w
		end,
		bounds = function(want: number, least: number, most: number)
			table.insert(seen, { want, least, most })
		end,
	}
	-- Not pressed: a move changes nothing.
	drag.move(100)
	expect(kept).to_be(480)
	drag.press(800)
	expect(drag.held()).to_be(true)
	-- Left 100: 100 wider.
	drag.move(700)
	expect(kept).to_be(580)
	-- Far left: as wide as leaves the conversation its least (1200 - 361).
	drag.move(100)
	expect(kept).to_be(839)
	expect(seen[#seen][1]).to_be(1180)
	expect(seen[#seen][2]).to_be(300)
	expect(seen[#seen][3]).to_be(839)
	-- Far right: its least.
	drag.move(1150)
	expect(kept).to_be(300)
	-- Back where it started, from the press (not the last move).
	drag.move(800)
	expect(kept).to_be(480)
	drag.release()
	expect(drag.held()).to_be(false)
	drag.move(600)
	expect(kept).to_be(480)
	-- A narrower page holds a press's width within it too.
	page = 700
	drag.press(500)
	drag.move(400)
	expect(kept).to_be(339)
	drag.release()
	-- A double press: back to its share (nothing kept).
	drag.press(500, 2)
	expect(kept).to_be_nil()
	expect(drag.held()).to_be(false)
end)
