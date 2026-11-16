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

-- What each tab builds: its own content's marks.
local OWN = {
	overview = { "#control-columns", "#control-capture-composer", "#control-widget-needs" },
	board = { "#control-board-tab", "#control-board-done" },
	chat = { "#control-chat", "#lane-control-chat" },
}

local function only(app: any, tab: string)
	for id, marks in OWN do
		for _, m in marks do
			expect(app:has(m)).to_be(id == tab)
		end
	end
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
	only(app, "board")
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
	expect(shell:has("#control-chat")).to_be(true)
end)

test("control tabs: each tab builds its own content and nothing of the others", function()
	local app = open()
	only(app, "overview")
	for _, tab in { "board", "chat", "overview", "chat", "board" } do
		app:set_state("control_tab", tab)
		app:frames(30)
		only(app, tab)
	end
	-- (The Board and the widgets' menu: the Board is a tab, never a widget.)
	expect(app:has("#control-widget-board")).to_be(false)
	expect(app:error()).to_be_nil()
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
	expect(app:has("#control-float")).to_be(false)
	app:type(CAPTURE_FIELD, "half a thought")
	app:frames(5)
	for _, tab in { "board", "chat" } do
		app:set_state("control_tab", tab)
		app:frames(30)
		expect(app:has("#control-capture-composer")).to_be(false)
		expect(app:has("#control-float")).to_be(true)
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
	expect(app:has("#control-float")).to_be(false)
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
	expect(app:count("#control-room >> role:textbox")).to_be(1)
end)

test("control tabs: Chat reads the Brain dump's transcript in the chat view, watched only while it shows", function()
	local app = open()
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
	-- Left: let go.
	app:set_state("control_tab", "board")
	app:frames(30)
	expect(watched(app)["control-chat"]).to_be_nil()
	expect(app:has("#lane-control-chat")).to_be(false)
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
	expect(calls(app)).to_contain("upsertItem")
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
