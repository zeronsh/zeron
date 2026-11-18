-- The Control room's tabs (screens/control/page): Overview, Board and Chat,
-- the one shown kept on this device (`control_tab`), ⌘⌥1–3. Only the tab
-- shown is built; Capture's full composer is Overview's alone, the floating
-- one is on every tab (Overview's once Capture is scrolled away), one draft
-- in both; Chat reads the Brain dump's transcript through the chat view's
-- own (screens/chat/lane_tab, ds/transcript), watched only while it shows.
-- On ui/control_page_fixture.luau (ui/control_world), and in the shell.

local DRAFT = "chat_lane_draft_orc-brain"
local CAPTURE_FIELD = "#control-capture-composer >> role:textbox"
local FLOAT_FIELD = "#control-float >> role:textbox"

local function open(fixture: string?): any
	local app = mount(fixture or "control_page_fixture.luau")
	app:resize(1440, 900)
	app:frames(30)
	return app
end

local function calls(app: any): string
	return tostring(app:state("calls") or "")
end

local function click_tab(app: any, label: string)
	app:click(`#control-tabs >> "{label}"`)
	app:frames(30)
end

local function watched(app: any): { [string]: string }
	return app:state("watched") or {}
end

-- A kept tab (screens/control/page's `kept`): built, and shown or hidden.
local function built(app: any, tab: string): boolean
	return app:has(`#control-tab-{tab}`)
end
local function first_line(app: any, target: string): string
	return string.match(app:tree(target, 0), "[^\n]+") or ""
end
local function shown(app: any, tab: string): boolean
	return built(app, tab) and not string.find(first_line(app, `#control-tab-{tab}`), " hidden", 1, true)
end
-- The floating composer shows (kept built, hidden where it doesn't float).
local function floats(app: any): boolean
	return not string.find(first_line(app, "#control-float-layer"), " hidden", 1, true)
end
-- A node's id in the tree (`@12`): the same while it isn't built again.
local function node_id(app: any, target: string): string
	return string.match(first_line(app, target), "^@(%d+)") or ""
end

-- ds/stage's background pieces still waiting, and the run counts.
local function perf(app: any): ({ [string]: number }, number)
	app:command("fixture.perf_read")
	return app:state("perf_counts") or {}, app:state("stage_idle_waiting") or 0
end
-- Until the page has settled and built the other tabs in the background.
local function settled(app: any)
	expect(app:wait("#control-tab-board", { timeout_ms = 5000 })).to_be(true)
	for _ = 1, 4000 do
		local _, waiting = perf(app)
		if waiting == 0 and built(app, "chat") then
			break
		end
		app:frames(1)
	end
	local _, waiting = perf(app)
	expect(waiting).to_be(0)
	app:frames(10)
end

test("control tabs: Overview first; a click on each shows it, kept as control_tab", function()
	local app = open()
	expect(app:state("control_tab")).to_be("overview")
	expect(app:has('#control-tabs >> "Overview"')).to_be(true)
	expect(app:has('#control-tabs >> "Board"')).to_be(true)
	expect(app:has('#control-tabs >> "Chat"')).to_be(true)
	click_tab(app, "Board")
	expect(app:state("control_tab")).to_be("board")
	click_tab(app, "Chat")
	expect(app:state("control_tab")).to_be("chat")
	click_tab(app, "Overview")
	expect(app:state("control_tab")).to_be("overview")
	expect(app:error()).to_be_nil()
end)

test("control tabs: the tab kept is the one the page opens on", function()
	local app = open()
	app:set_state("control_tab", "board")
	app:frames(30)
	expect(shown(app, "board")).to_be(true)
	expect(shown(app, "overview")).to_be(false)
	-- (The shell's Control room, made again on coming back to it.)
	local shell = open("control_fixture.luau")
	shell:set_state("control_tab", "chat")
	shell:frames(10)
	shell:command("fixture.leave")
	shell:frames(30)
	expect(shell:has("#control-room")).to_be(false)
	shell:command("fixture.control")
	shell:frames(40)
	expect(shell:has("#control-room")).to_be(true)
	expect(shown(shell, "chat")).to_be(true)
end)

test("control tabs: the tab shown builds first; the others in the background once the page settles, hidden", function()
	local app = open()
	-- At first: Overview alone.
	expect(shown(app, "overview")).to_be(true)
	expect(built(app, "board")).to_be(false)
	expect(built(app, "chat")).to_be(false)
	settled(app)
	-- Then the others, built and hidden: their content there, not shown.
	expect(shown(app, "overview")).to_be(true)
	expect(built(app, "board") and not shown(app, "board")).to_be(true)
	expect(built(app, "chat") and not shown(app, "chat")).to_be(true)
	expect(app:has("#control-tab-board >> #control-board-done")).to_be(true)
	expect(app:has("#control-tab-chat >> #lane-control-chat")).to_be(true)
	-- (The Board is a tab, never a widget.)
	expect(app:has("#control-widget-board")).to_be(false)
	expect(app:error()).to_be_nil()
end)

test("control tabs: switching shows a kept tab, never builds it again", function()
	local app = open()
	settled(app)
	local ids = {
		board = node_id(app, "#control-board-tab"),
		chat = node_id(app, "#lane-control-chat"),
		overview = node_id(app, "#control-columns"),
		float = node_id(app, "#control-float"),
	}
	for _, tab in { "board", "chat", "overview", "chat", "board", "overview" } do
		app:set_state("control_tab", tab)
		app:frames(30)
		for id, t in { overview = "overview", board = "board", chat = "chat" } do
			expect(shown(app, t)).to_be(id == tab)
		end
	end
	expect(node_id(app, "#control-board-tab")).to_be(ids.board)
	expect(node_id(app, "#lane-control-chat")).to_be(ids.chat)
	expect(node_id(app, "#control-columns")).to_be(ids.overview)
	expect(node_id(app, "#control-float")).to_be(ids.float)
	local _, waiting = perf(app)
	expect(waiting).to_be(0)
	expect(app:error()).to_be_nil()
end)

test("control tabs: a hidden tab does no work while what it shows changes; shown, it catches up", function()
	local app = open()
	settled(app)
	app:set_state("control_tab", "chat")
	app:frames(40)
	app:command("fixture.perf_reset")
	for _ = 1, 3 do
		app:command("fixture.push_all")
		app:frames(20)
	end
	local counts = perf(app)
	-- Nothing of Overview's or the Board's read again, built or drawn.
	for _, name in { "hold.overview", "hold.board", "board.word", "board.status", "progress.row", "progress.clocks", "graph.layout", "graph.draw", "hold.progress" } do
		expect(counts[name] or 0).to_be(0)
	end
	-- Shown: it reads what changed.
	app:set_state("control_tab", "board")
	app:frames(40)
	counts = perf(app)
	expect((counts["hold.board"] or 0) > 0).to_be(true)
	expect(app:error()).to_be_nil()
end)

test("control tabs: the background build is one piece a frame, out of layout till it's done", function()
	local app = open()
	expect(app:wait("#control-tab-board", { timeout_ms = 5000 })).to_be(true)
	local _, last = perf(app)
	local most = 0
	local saw_hidden = false
	for _ = 1, 4000 do
		app:frames(1)
		local _, waiting = perf(app)
		-- (A piece built makes the pieces it holds: only ever one fewer.)
		if waiting < last then
			most = math.max(most, last - waiting)
		end
		if waiting > 0 and app:has("#control-tab-board >> [1]") then
			saw_hidden = saw_hidden or string.find(first_line(app, "#control-tab-board >> [1]"), " hidden", 1, true) ~= nil
		end
		last = waiting
		if waiting == 0 and built(app, "chat") then
			break
		end
	end
	expect(last).to_be(0)
	expect(most).to_be(1)
	expect(saw_hidden).to_be(true)
	expect(app:error()).to_be_nil()
end)

test("control tabs: a tab opened before its background build is done builds at once", function()
	local app = open()
	expect(app:wait("#control-tab-board", { timeout_ms = 5000 })).to_be(true)
	app:set_state("control_tab", "board")
	app:frames(30)
	expect(shown(app, "board")).to_be(true)
	-- (Its cards, every column's: Done's shows only what was done since
	-- midnight, empty in the small hours whatever the sample says.)
	expect(app:count("#control-board-tab >> .orch-card") > 3).to_be(true)
	expect(app:count("#control-tab-board >> .stage-idle")).to_be(0)
end)

test("control tabs: ⌘⌥1–3 show Overview, Board and Chat", function()
	local app = open("control_fixture.luau")
	app:command("control.tab_board")
	app:frames(30)
	expect(app:state("control_tab")).to_be("board")
	expect(app:has("#control-board-tab")).to_be(true)
	app:command("control.tab_chat")
	app:frames(30)
	expect(app:state("control_tab")).to_be("chat")
	app:command("control.tab_overview")
	app:frames(30)
	expect(app:state("control_tab")).to_be("overview")
	expect(app:has("#control-capture-composer")).to_be(true)
end)

test("control tabs: the full composer on Overview only, the floating one on Board and Chat, one draft", function()
	local app = open()
	expect(app:has("#control-capture-composer")).to_be(true)
	expect(floats(app)).to_be(false)
	app:type(CAPTURE_FIELD, "half a thought")
	app:frames(5)
	for _, tab in { "board", "chat" } do
		app:set_state("control_tab", tab)
		app:frames(30)
		-- (Overview's Capture kept, hidden with it.)
		expect(shown(app, "overview")).to_be(false)
		expect(floats(app)).to_be(true)
		-- (Bare: one line and its knob.)
		expect(app:state("control_capture_float_open")).to_be(false)
		expect(app:text(FLOAT_FIELD)).to_be("half a thought")
	end
	-- Typed into on Chat, the same draft in Capture back on Overview.
	app:type(FLOAT_FIELD, " and another")
	app:frames(5)
	expect(app:state(DRAFT)).to_be("half a thought and another")
	app:set_state("control_tab", "overview")
	app:frames(30)
	expect(app:text(CAPTURE_FIELD)).to_be("half a thought and another")
	-- (Capture in view: nothing floats.)
	app:frames(30)
	expect(floats(app)).to_be(false)
	expect(app:error()).to_be_nil()
end)

test("control tabs: ⌘⇧K on Board gives the floating composer the keys", function()
	local app = open("control_fixture.luau")
	app:set_state("control_tab", "board")
	app:frames(30)
	app:command("control.focus_capture")
	app:frames(10)
	local focused = false
	for line in app:tree("#control-float [1]"):gmatch("[^\n]+") do
		if line:find("textbox", 1, true) and line:find(" focused", 1, true) then
			focused = true
		end
	end
	expect(focused).to_be(true)
end)

test("control tabs: Chat sends from the floating composer as Capture does, a dump", function()
	local app = open()
	app:set_state("control_tab", "chat")
	app:frames(30)
	app:type(FLOAT_FIELD, "route this please")
	app:key("Enter")
	app:frames(5)
	expect(calls(app)).to_contain("dump:route this please")
	-- (No composer of the chat's own: the floating one is the only one.)
	expect(app:has("#lane-composer-control-chat")).to_be(false)
	expect(app:count("#control-tab-chat >> role:textbox")).to_be(0)
end)

test("control tabs: Chat reads the Brain dump's transcript in the chat view; hidden, its reply held; let go with the page", function()
	local app = open()
	-- (Not watched till the tab is built: shown, or in the background.)
	expect(watched(app)["control-chat"]).to_be_nil()
	app:set_state("control_tab", "chat")
	app:frames(40)
	expect(watched(app)["control-chat"]).to_be("orc-brain")
	-- The chat view's transcript (ds/transcript), read-only: the person's
	-- dumps, the routing turn, its tool calls.
	expect(app:has("#control-chat >> .read-tab")).to_be(true)
	expect(app:has("#lane-control-chat >> #transcript")).to_be(true)
	local function reads(words: string): boolean
		return string.find(app:tree("#lane-control-chat"), words, 1, true) ~= nil
	end
	expect(app:has("#lane-control-chat >> #bd-u1")).to_be(true)
	expect(reads("Moscow node is flaky again")).to_be(true)
	expect(reads("Routed two parts")).to_be(true)
	expect(reads("Also check the relay window with Wing.")).to_be(true)
	-- Streaming: a reply coming shows as it grows.
	app:set_state("chat.lane.control-chat.messages", {
		{ id = "bd-u1", role = "user", text = "Moscow node is flaky again, and the board needs a Review column." },
		{ id = "bd-u2", role = "user", text = "Also check the relay window with Wing." },
		{ id = "bd-a2", role = "assistant", text = "", streaming = true },
	})
	app:set_state("chat.lane.control-chat.draft", { id = "bd-a2", text = "Asking Wing about the relay" })
	app:frames(30)
	expect(reads("Asking Wing about the relay")).to_be(true)
	-- Hidden: still watched (shown again, nothing to load), the reply
	-- being written held where it was.
	app:set_state("control_tab", "board")
	app:frames(30)
	expect(watched(app)["control-chat"]).to_be("orc-brain")
	app:command("fixture.perf_reset")
	app:set_state("chat.lane.control-chat.draft", { id = "bd-a2", text = "Asking Wing about the relay window now" })
	app:frames(20)
	expect(perf(app)["hold.chat"] or 0).to_be(0)
	expect(reads("relay window now")).to_be(false)
	-- Shown: caught up.
	app:set_state("control_tab", "chat")
	app:frames(30)
	expect(reads("relay window now")).to_be(true)
	expect(app:error()).to_be_nil()
end)

test("control tabs: the Board tab moves a card by drag and by Move to", function()
	local app = open()
	app:set_state("control_tab", "board")
	app:frames(40)
	-- The page's width: every column, side by side.
	expect(app:has("#control-board-done")).to_be(true)
	app:drag('#control-board-tab >> .orch-card "Reboot Moscow node"', "#control-board-blocked")
	app:frames(20)
	-- (Into Blocked: paused.)
	expect(calls(app)).to_contain("pauseItem")
	local before = #calls(app)
	app:right_click('#control-board-tab >> role:button "Phase 2 migration"')
	app:frames(10)
	app:click('role:menuitem "Move to"')
	app:frames(10)
	app:click('role:menuitem "Review"')
	app:frames(20)
	expect(#calls(app) > before).to_be(true)
	expect(string.sub(calls(app), before + 1)).to_contain("upsertItem")
	expect(app:error()).to_be_nil()
end)

test("control tabs: the Chat tab's transcript is let go as the Control room is left", function()
	local app = open("control_fixture.luau")
	app:set_state("control_tab", "chat")
	app:frames(40)
	expect(watched(app)["control-chat"]).to_be("orc-brain")
	app:command("fixture.leave")
	app:frames(30)
	expect(watched(app)["control-chat"]).to_be_nil()
end)

test("control tabs: the tab buttons stand in the same place on every tab", function()
	local app = open()
	local function at(label: string): string
		local line = string.match(app:tree(`#control-tabs >> "{label}"`, 0), "[^\n]+") or ""
		return string.match(line, " (%-?[%d%.]+,%-?[%d%.]+) ") or ""
	end
	local function all(): string
		return `{at("Overview")} {at("Board")} {at("Chat")}`
	end
	local overview = all()
	expect(overview ~= "  ").to_be(true)
	for _, tab in { "board", "chat", "overview", "chat" } do
		app:set_state("control_tab", tab)
		app:frames(30)
		expect(all()).to_be(overview)
	end
	-- (The widgets' menu, Overview's alone, to their left.)
	expect(app:has("#control-widgets-menu")).to_be(true)
end)

test("control page: the Map's default view, written out for the page to load without the Map, is the Map's own", function()
	local app = mount("control_page_map_default_fixture.luau")
	app:frames(2)
	expect(app:state("map_default_same")).to_be(true)
end)
