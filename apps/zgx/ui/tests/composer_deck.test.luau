-- The deck, on ui/running_tray_fixture.luau: folded, a pill floating over
-- the composer (each thing waiting its glyph and count); open, the
-- composer's own top, its header tabs (one alone, a title).

local function tasks(n: number): { any }
	local list = {}
	for i = 1, n do
		table.insert(list, { id = `b{i}`, kind = "process", name = `job {i}`, since = os.time() - 60 * i })
	end
	return list
end

-- Whether a node is on screen (a folded top keeps its rows laid out of the
-- way, hidden).
local function shown(app: any, what: string): boolean
	for line in app:tree():gmatch("[^\n]+") do
		if line:find(what, 1, true) then
			return not line:find(" hidden", 1, true) and not line:find(" 0x0", 1, true)
		end
	end
	return false
end

test("folded, a pill says what waits; a tap on it opens that one", function()
	local app = mount("running_tray_fixture.luau")
	app:resize(700, 600)
	app:frames(10)
	app:set_state("test_running_tasks", tasks(2))
	app:frames(30)
	expect(app:has('role:toolbar "What\'s waiting"')).to_be(true)
	expect(app:has('role:button "Open Running"')).to_be(true)
	expect(shown(app, 'tab "Open Running"')).to_be(false)
	app:click('role:button "Open Running"')
	app:frames(30)
	-- Open: the pill goes, the top shows its tab and rows.
	expect(app:state("test_running_tray")).to_be("tasks")
	expect(app:has('role:toolbar "What\'s waiting"')).to_be(false)
	expect(shown(app, 'tab "Fold Running"')).to_be(true)
	expect(app:has('role:button "Stop job 2"')).to_be(true)
	expect(app:error()).to_be(nil)
end)

test("subagents and processes are tabs of their own, one open at a time", function()
	local app = mount("running_tray_fixture.luau")
	app:resize(700, 600)
	app:frames(10)
	local list = tasks(1)
	table.insert(list, 1, { id = "a1", kind = "agent", name = "scan the repo" })
	app:set_state("test_running_tasks", list)
	app:frames(30)
	expect(app:has('role:button "Open Subagents"')).to_be(true)
	expect(app:has('role:button "Open Running"')).to_be(true)
	app:click('role:button "Open Subagents"')
	app:frames(30)
	expect(app:has('role:button "Open scan the repo"')).to_be(true)
	expect(app:has('role:button "Stop job 1"')).to_be(false)
	-- Another tab: its rows, the other's gone.
	app:click('role:tab "Open Running"')
	app:frames(30)
	expect(app:state("test_running_tray")).to_be("tasks")
	expect(app:has('role:button "Stop job 1"')).to_be(true)
	expect(app:has('role:button "Open scan the repo"')).to_be(false)
	-- The open tab again folds it, back to the pill.
	app:click('role:tab "Fold Running"')
	app:frames(30)
	expect(app:state("test_running_tray")).to_be(nil)
	expect(app:has('role:toolbar "What\'s waiting"')).to_be(true)
	expect(app:error()).to_be(nil)
end)

test("the chevrons: the header's folds, the pill's opens the one open last", function()
	local app = mount("running_tray_fixture.luau")
	app:resize(700, 600)
	app:frames(10)
	app:set_state("test_running_tasks", tasks(1))
	app:set_state("test_deck_todos", { { id = "t1", text = "port the deck", state = "doing" }, { id = "t2", text = "test it", state = "pending" } })
	app:frames(30)
	-- Nothing opens itself: the plan arriving is in the pill.
	expect(app:state("test_running_tray")).to_be(nil)
	expect(app:has('role:button "Open Todo"')).to_be(true)
	app:click('role:button "Open Running"')
	app:frames(30)
	app:click('role:button "Fold what\'s waiting"')
	app:frames(30)
	expect(app:state("test_running_tray")).to_be(nil)
	app:click('role:button "Show what\'s waiting"')
	app:frames(30)
	expect(app:state("test_running_tray")).to_be("tasks")
	expect(app:error()).to_be(nil)
end)

-- How many alert rows wear a glyph of their own (not a button's, within them).
local function row_glyphs(app: any): number
	local n = 0
	for line in app:tree('role:list "Alerts"'):gmatch("[^\n]+") do
		if line:match("^    @%d+ img") and not line:find(" hidden", 1, true) then
			n += 1
		end
	end
	return n
end

-- How far in from its row an alert's words start. (It needs a word more for the
-- tree to list them apart from its row.)
local function words_in(app: any, title: string): number
	local tree = app:tree('role:list "Alerts"')
	local row = string.match(tree, `listitem "{title}[^"]*" (%d+),`)
	local words = string.match(tree, `text "{title}" (%d+),`)
	return (tonumber(words) :: number) - (tonumber(row) :: number)
end

test("an alert alone has one glyph, the tab's; with others, each its own", function()
	local app = mount("running_tray_fixture.luau")
	app:resize(700, 600)
	app:frames(10)
	local context = { id = "al1", tone = "warning", title = "Context nearly full", detail = "a word more" }
	app:set_state("test_deck_alerts", { context })
	app:frames(30)
	app:click('role:button "Open Alerts"')
	app:frames(30)
	expect(row_glyphs(app)).to_be(0)
	-- Its words stand where the glyph would have been, not after a gap.
	expect(words_in(app, "Context nearly full") < 16).to_be(true)
	app:set_state("test_deck_alerts", { context, { id = "al2", tone = "danger", title = "github MCP server failed to start" } })
	app:frames(30)
	expect(row_glyphs(app)).to_be(2)
	expect(words_in(app, "Context nearly full") > 16).to_be(true)
	expect(app:error()).to_be(nil)
end)

test("an alert alone that's still working keeps its ring: the tab's glyph doesn't say so", function()
	local app = mount("running_tray_fixture.luau")
	app:resize(700, 600)
	app:frames(10)
	app:set_state("test_deck_alerts", { { id = "al1", tone = "info", title = "Reconnecting", detail = "a word more", spinner = true } })
	app:frames(30)
	app:click('role:button "Open Alerts"')
	app:frames(30)
	expect(words_in(app, "Reconnecting") > 16).to_be(true)
	expect(app:error()).to_be(nil)
end)

test("a quiet alert raises no pill of its own, and is read with the rest", function()
	local app = mount("running_tray_fixture.luau")
	app:resize(700, 600)
	app:frames(10)
	local asleep = { id = "q1", tone = "info", title = "Machine asleep", quiet = true }
	app:set_state("test_deck_alerts", { asleep })
	app:frames(30)
	expect(app:has('role:button "Open Alerts"')).to_be(false)
	app:set_state("test_deck_alerts", { asleep, { id = "al1", tone = "warning", title = "Disk nearly full" } })
	app:frames(30)
	-- Counted: only what raises it.
	expect(app:has('role:button "Open Alerts" >> "1"')).to_be(true)
	app:click('role:button "Open Alerts"')
	app:frames(30)
	expect(app:has('role:listitem ~"Machine asleep"')).to_be(true)
	expect(app:has('role:listitem ~"Disk nearly full"')).to_be(true)
	-- The loud one gone, the tab goes with it.
	app:set_state("test_deck_alerts", { asleep })
	app:frames(30)
	expect(app:state("test_running_tray")).to_be(nil)
	expect(app:has('role:button "Open Alerts"')).to_be(false)
	expect(app:error()).to_be(nil)
end)

test("a held row waits: only Remove, its action's place saying why", function()
	local app = mount("running_tray_fixture.luau")
	app:resize(700, 600)
	app:frames(10)
	app:set_state("test_deck_queue", { { id = "h1", text = "Held words", waiting = "Waits for a size", age = "2 days ago" } })
	app:frames(30)
	app:click('role:button "Open Queued"')
	app:frames(30)
	local row = 'role:listitem ~"Held words"'
	expect(app:tree(`{row} >> role:button "Waits for a size"`)).to_contain("disabled")
	expect(app:tree(`{row} >> role:button "Edit"`)).to_contain("disabled")
	expect(app:tree(`{row} >> role:button "Remove"`)).never.to_contain("disabled")
	expect(app:has(`{row} >> "2 days ago"`)).to_be(true)
	expect(app:error()).to_be(nil)
end)

test("alerts: first in the pill, each with its action and its dismiss", function()
	local app = mount("running_tray_fixture.luau")
	app:resize(700, 600)
	app:frames(10)
	app:set_state("test_running_tasks", tasks(1))
	app:set_state("test_deck_alerts", {
		{ id = "al1", tone = "danger", title = "github MCP server failed to start", action = "Retry" },
		{ id = "al2", tone = "warning", title = "Context 92% full" },
	})
	app:frames(30)
	-- In the pill, never opened for the person.
	expect(app:has('role:button "Open Alerts"')).to_be(true)
	expect(app:state("test_running_tray")).to_be(nil)
	app:click('role:button "Open Alerts"')
	app:frames(30)
	expect(app:state("test_running_tray")).to_be("alerts")
	app:click('role:button "Retry"')
	app:click('role:button "Dismiss Context 92% full"')
	app:frames(5)
	expect(app:state("calls")).to_contain("alert:al1")
	expect(app:state("calls")).to_contain("dismiss:al2")
	-- None left: its tab goes, and folds if it was open.
	app:set_state("test_deck_alerts", {})
	app:frames(30)
	expect(app:state("test_running_tray")).to_be(nil)
	expect(app:has('role:button "Open Alerts"')).to_be(false)
	expect(app:error()).to_be(nil)
end)

-- A box from the test tree's first line under `target`: x, y, w, h.
local function box(app: any, target: string): (number, number, number, number)
	local line = string.match(app:tree(target), "[^\n]+") or ""
	local x, y, w, h = line:match(" (%-?[%d.]+),(%-?[%d.]+) ([%d.]+)x([%d.]+)")
	return tonumber(x) or 0, tonumber(y) or 0, tonumber(w) or 0, tonumber(h) or 0
end

test("the pill's capsules sit the same inset in from all four of its sides", function()
	local app = mount("running_tray_fixture.luau")
	app:resize(700, 600)
	app:frames(10)
	app:set_state("test_running_tasks", tasks(2))
	app:frames(30)
	local px, py, pw, ph = box(app, 'role:toolbar "What\'s waiting"')
	local cx, cy, _, ch = box(app, 'role:button "Open Running"')
	local ex, _, ew = box(app, 'role:button "Show what\'s waiting"')
	local top, bottom = cy - py, (py + ph) - (cy + ch)
	-- (Its glass's border is laid out inside it: the inset counts it.)
	expect(top).to_be(bottom)
	expect(cx - px).to_be(top)
	expect((px + pw) - (ex + ew)).to_be(top)
end)
