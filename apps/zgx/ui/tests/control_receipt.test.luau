-- The Control room's Capture (screens/control/page, ds/orch_receipt): the
-- newest dump's parts underlined with a chip each, a new dump's parts lit
-- one after another as they're placed, and an unsure part placed from its
-- likely places. On ui/control_page_fixture.luau (orchestration_sample).

local function open(): any
	local app = mount("control_page_fixture.luau")
	app:resize(1440, 900)
	app:frames(30)
	return app
end

local function calls(app: any): string
	return tostring(app:state("calls") or "")
end

test("the newest dump's parts each have a chip saying where they went", function()
	local app = open()
	expect(app:error()).to_be_nil()
	-- Its text, whole (its label), each part's words under the pointer's
	-- reach.
	expect(app:has('#control-capture >> .orch-receipt-text >> "registry "')).to_be(true)
	expect(app:count("#control-capture >> .orch-part") >= 5).to_be(true)
	expect(app:has('#control-capture >> role:button "Orchestrator screen, Context, Done"')).to_be(true)
	expect(app:has('#control-capture >> role:button "Later, Task, Delivered"')).to_be(true)
	expect(app:has('#control-capture >> role:button "Exnomic VPN, Question, Answered"')).to_be(true)
	expect(app:has('#control-capture >> role:button "CI speed, Priority, Delivered"')).to_be(true)
	expect(app:count("#control-capture >> .orch-chip")).to_be(5)
	-- There before the page: every part lit at once.
	expect((app:state("orch_receipt_lit") or {})["dp-2"]).to_be(5)
end)

test("a question's answer comes back as a reply line, with a way into its thread", function()
	local app = open()
	-- There without a click: the answer, quiet, under the chips.
	expect(app:has('#control-capture >> .orch-answer >> ~"waiting on your approval"')).to_be(true)
	-- (Its, and the done context part's outcome.)
	expect(app:count("#control-capture >> .orch-answer")).to_be(2)
	-- One line: the condensed answer, never the whole reply.
	expect(app:has('#control-capture >> .orch-answer >> ~"wg show"')).to_be(false)
	app:click('#control-capture >> .orch-answer-open [2]')
	app:frames(5)
	expect(app:error()).to_be_nil()
end)

test("a new dump is sent with Enter and its parts light one after another", function()
	local app = open()
	app:type("#control-capture >> role:textbox", "fix the flaky sync test and ask vpn about moscow")
	app:key("Enter")
	app:frames(5)
	expect(calls(app)).to_contain("dump:fix the flaky sync test and ask vpn about moscow")
	expect(app:text("#control-capture >> role:textbox")).to_be("")
	-- Routing: nothing placed yet.
	expect(app:count("#control-capture >> .orch-chip")).to_be(0)
	app:command("fixture.route_newest")
	app:frames(1)
	local lit = app:state("orch_receipt_lit") or {}
	local id: string? = nil
	for k in lit do
		if string.find(k, "^dp%-new") then
			id = k
		end
	end
	expect(id ~= nil).to_be(true)
	-- Not all at once: the first, then the second about 60 ms on.
	expect(lit[id :: string] < 2).to_be(true)
	expect(app:wait('#control-capture >> role:button "Exnomic VPN, Question, Pending"')).to_be(true)
	local deadline = os.clock() + 2
	while ((app:state("orch_receipt_lit") or {})[id :: string] or 0) < 2 and os.clock() < deadline do
		app:frames(5)
	end
	expect((app:state("orch_receipt_lit") or {})[id :: string]).to_be(2)
end)

test("an unsure part is placed from its likely places", function()
	local app = open()
	app:click("#control-history-toggle")
	app:frames(10)
	app:click(".control-history-row")
	app:frames(20)
	expect(app:has('#control-capture >> role:button "Unsure, Task, Unsure"')).to_be(true)
	app:click('#control-capture >> .orch-candidate "Orchestrators"')
	app:frames(10)
	expect(calls(app)).to_contain("reroute:dp-1:rt-2:orchestrator:orc-orch")
	expect(app:has('#control-capture >> role:button "Orchestrators, Task, Delivered"')).to_be(true)
	-- A new thread, not made yet, goes with an empty id.
	expect(app:has('#control-capture >> .orch-candidate')).to_be(false)
end)

-- The open menu's items, as the tree shows them (a closed menu's leave
-- hidden).
local function menu_items(app: any): { string }
	local out = {}
	for line in app:tree():gmatch("[^\n]+") do
		local name = line:match(' menuitem "([^"]*)"')
		if name and not line:find("hidden", 1, true) then
			table.insert(out, name)
		end
	end
	return out
end

-- A box as the tree shows it (whole points): x, y, w, h; the first line
-- holding `pattern`.
local function box_of(app: any, pattern: string): (number, number, number, number)
	for line in app:tree():gmatch("[^\n]+") do
		if line:find(pattern) and not line:find("hidden", 1, true) then
			local x, y, w, h = line:match(" (%-?%d+),(%-?%d+) (%d+)x(%d+)")
			return tonumber(x) or 0, tonumber(y) or 0, tonumber(w) or 0, tonumber(h) or 0
		end
	end
	return 0, 0, 0, 0
end

test("each chip's menu opens from a secondary click on it, with places to move it", function()
	local app = open()
	-- An odd width: the column centred, a chip starts between points.
	app:resize(2023, 1102)
	app:frames(30)
	local n = app:count("#control-capture >> .orch-chip")
	expect(n).to_be(5)
	for i = 1, n do
		-- (Each from a fresh page, as the unsure chip's below: a menu opened
		-- right after another's closed can still show its items hidden in
		-- the tree, the shell's and this page's alike.)
		app = open()
		app:resize(2023, 1102)
		app:frames(30)
		local chip = `#control-capture >> .orch-chip [{i}]`
		local name = string.match(app:tree(chip), 'button "([^"]*)"') or ""
		local _, cy, _, ch = box_of(app, `button "{name}"`)
		-- (A primary click pins the part's card: ds/hover_card Anchored.)
		app:right_click(chip)
		-- (Its items fade in: settled, with the page's composer about.)
		app:frames(30)
		expect(app:error()).to_be_nil()
		local items = menu_items(app)
		expect(#items > 0).to_be(true)
		expect(table.concat(items, ",")).to_contain("New thread")
		-- Its panel a size, at the pointer on the chip.
		local _, my, mw, mh = box_of(app, ' menu "')
		expect(mw > 0 and mh > 0).to_be(true)
		expect(my >= cy and my <= cy + ch).to_be(true)
		app:key("Escape")
		app:frames(5)
	end
	-- An unsure part's chip too.
	app = open()
	app:resize(2023, 1102)
	app:frames(30)
	app:click("#control-history-toggle")
	app:frames(10)
	app:click(".control-history-row")
	app:frames(20)
	app:right_click('#control-capture >> role:button "Unsure, Task, Unsure"')
	app:frames(5)
	expect(table.concat(menu_items(app), ",")).to_contain("Note")
	expect(app:error()).to_be_nil()
end)

test("a context menu hangs on whole points and opens only with something to choose", function()
	local menu_point = require("@ds/menu_point")
	-- A chip at 288.5 (a centred column of an odd width), the menu 4 under
	-- it: on 289, in its box at 280.25.
	local x, y = menu_point.at(288.5, 353, 280.25, 300)
	expect(x + 280.25).to_be(289)
	expect(y + 300).to_be(353)
	expect(menu_point.whole(514.3)).to_be(514)
	expect(menu_point.whole(-0.5)).to_be(0)
	expect(menu_point.has_items(nil)).to_be(false)
	expect(menu_point.has_items({})).to_be(false)
	expect(menu_point.has_items({ { heading = "Move to" }, { separator = true } })).to_be(false)
	expect(menu_point.has_items({ { heading = "Move to" }, { label = "Note" } })).to_be(true)
end)

test("the page draws and resizes without a slip", function()
	local app = open()
	for _, size in { { 900, 700 }, { 1440, 900 }, { 700, 900 }, { 1800, 1100 } } do
		app:resize(size[1], size[2])
		app:frames(10)
		expect(app:error()).to_be_nil()
	end
end)

test("a dump's text is cut at its parts' spans, each underlined part naming its route", function()
	local data = require("@ds/orch_data")
	local sample = require("@account/orchestration_sample")
	local d = sample.now.dumps[1]
	local segs = data.segments(d.text, d.routes)
	local whole, routed = {}, {}
	for _, s in segs do
		table.insert(whole, s.text)
		if s.route then
			routed[s.route] = s.text
		end
	end
	-- Nothing lost or doubled, and each part exactly its excerpt.
	expect(table.concat(whole)).to_be(d.text)
	for i, r in d.routes do
		expect(routed[i]).to_be(r.excerpt)
	end
	-- A part with no span is found by its words; one past the text is left out.
	local found = data.segments("alpha beta gamma", { { excerpt = "beta" }, { span = { from = 10, to = 99 }, excerpt = "x" } })
	expect(#found).to_be(3)
	expect(found[2].text).to_be("beta")
	expect(found[2].route).to_be(1)
end)

-- A node's box from the tree: x, y, w, h.
local function box(app: any, target: string): (number, number, number, number)
	local line = app:tree(target):match("[^\n]+") or ""
	local x, y, w, h = line:match(" (%-?[%d%.]+),(%-?[%d%.]+) ([%d%.]+)x([%d%.]+)")
	return tonumber(x) or -1, tonumber(y) or -1, tonumber(w) or -1, tonumber(h) or -1
end
local LAST = '#orch-chips >> role:button "Engine: orchestration state, Question, Delivered"'
local FIRST = '#orch-chips >> role:button "Orchestrator screen, Context, Done"'

test("the chips stand in one row at any width, never wrapping", function()
	local app = open()
	for _, size in { { 1440, 900 }, { 900, 900 }, { 700, 900 }, { 520, 900 } } do
		app:resize(size[1], size[2])
		app:frames(10)
		local _, sy, _, sh = box(app, "#orch-chips")
		expect(sh < 40).to_be(true)
		local _, fy = box(app, FIRST)
		local _, ly = box(app, LAST)
		expect(fy).to_be(ly)
		expect(fy >= sy).to_be(true)
	end
end)

test("the chips scroll sideways past the room: dragged across", function()
	local app = open()
	app:resize(700, 900)
	app:frames(10)
	local before, by = box(app, FIRST)
	app:drag('#orch-chips >> role:button "CI speed, Priority, Delivered"', FIRST)
	app:frames(10)
	local after, ay = box(app, FIRST)
	-- Moved along, not down; and a drag opens no chip's menu.
	expect(after < before).to_be(true)
	expect(ay).to_be(by)
	expect(app:has('role:menuitem "Move to"')).to_be(false)
end)

test("the pointer on a part lights its chip and brings it into view", function()
	local app = open()
	app:resize(700, 900)
	app:frames(10)
	local vx, _, vw = box(app, "#orch-chips")
	local lx, _, lw = box(app, LAST)
	-- Past the strip's end at first.
	expect(lx + lw > vx + vw).to_be(true)
	app:hover('#control-capture >> .orch-part >> "registry "')
	app:frames(20)
	expect(app:state("orch_receipt_hovered")).to_be("rt-9")
	lx, _, lw = box(app, LAST)
	expect(lx >= vx and lx + lw <= vx + vw + 0.5).to_be(true)
	-- The pointer on a chip lights its part, as before.
	app:hover(LAST)
	app:frames(5)
	expect(app:state("orch_receipt_hovered")).to_be("rt-9")
	app:hover('#orch-chips >> role:button "CI speed, Priority, Delivered"')
	app:frames(5)
	expect(app:state("orch_receipt_hovered")).to_be("rt-8")
end)

test("a part's hover card: where it went, how it stands, what was sent, the answer, the way in", function()
	local app = mount("orch_part_card_fixture.luau")
	app:resize(1200, 600)
	app:frames(10)
	expect(app:has('#card-answered >> "Exnomic VPN"')).to_be(true)
	expect(app:has('#card-answered >> "Question"')).to_be(true)
	expect(app:has('#card-answered >> "Answered"')).to_be(true)
	expect(app:has('#card-answered >> ~"Did the Moscow node reboot cleanly?"')).to_be(true)
	-- The whole reply, its markdown drawn (no raw marks).
	expect(app:has('#card-answered >> .orch-part-answer >> ~"scheduled for 02:00 MSK"')).to_be(true)
	expect(app:has('#card-answered >> .orch-part-answer >> ~"wg show"')).to_be(true)
	expect(app:has('#card-answered >> .orch-part-answer >> ~"**"')).to_be(false)
	app:click("#card-answered >> .orch-part-open")
	app:frames(2)
	expect(app:state("opened")).to_be("orchestrator:orc-vpn")
	-- Unsure: no way in, its state said.
	expect(app:has('#card-unsure >> "Unsure"')).to_be(true)
	expect(app:has("#card-unsure >> .orch-part-open")).to_be(false)
end)

test("an answer's one line: the engine's own, or a whole reply's first sentence", function()
	local data = require("@ds/orch_data")
	expect(data.answer_line({ answer = "Short and `done`.", answer_full = "# Long\n\nall of it" })).to_be("Short and `done`.")
	expect(data.answer_line({ answer = "## Status\n- **Yes.** It ran `cargo test` fine. More after." })).to_be("Status **Yes.**")
	expect(data.answer_line({})).to_be("")
end)

test("the history: a row an earlier dump, opened in place", function()
	local app = open()
	app:click("#control-history-toggle")
	app:frames(10)
	expect(app:count(".control-history-row")).to_be(1)
	expect(app:has(".control-history-receipt")).to_be(false)
	app:click(".control-history-row")
	app:frames(10)
	expect(app:has('.control-history-receipt >> role:button "Unsure, Task, Unsure"')).to_be(true)
	-- The newest stays where it was, above.
	expect(app:has('#control-capture >> role:button "Exnomic VPN, Question, Answered"')).to_be(true)
end)

test("a part's hover card is as tall as what it holds, its reply scrolling only past its room", function()
	local app = mount("orch_part_card_fixture.luau")
	app:resize(1200, 700)
	app:frames(20)
	local _, cy, cw, ch = box(app, "#card-answered")
	-- Wide enough to read.
	expect(cw).to_be(480)
	-- Its last row's end, its inset and edge, and nothing more.
	local _, oy, _, oh = box(app, "#card-answered >> .orch-part-open")
	expect(math.abs((cy + ch) - (oy + oh + 12 + 1)) <= 1).to_be(true)
	-- The reply's scroll as tall as the reply (it fits), never past 280.
	local _, sy, _, sh = box(app, "#card-answered >> .orch-part-answer")
	expect(sh <= 280).to_be(true)
	local _, ly, _, lh = box(app, '#card-answered >> .orch-part-answer >> ~"report either way"')
	expect(math.abs((sy + sh) - (ly + lh)) <= 1).to_be(true)
	-- The message sent: one clamped run, cut with an ellipsis, not under the reply.
	local _, my, _, mh = box(app, "#card-answered >> .orch-part-message")
	expect(my + mh <= sy).to_be(true)
end)

-- Capture's composer: the chat composer (screens/chat/lane_tab's), bound to
-- the Brain dump's chat.
local COMPOSER = "#control-capture-composer"

test("capture is the chat composer: the Brain dump's, its model there", function()
	local app = open()
	expect(app:has(`{COMPOSER} >> role:textbox`)).to_be(true)
	-- (Its chat taken in once it's used: the pointer over it.)
	expect((app:state("watched") or {})["control-capture"]).to_be_nil()
	app:hover(`{COMPOSER} >> role:textbox`)
	app:frames(5)
	expect((app:state("watched") or {})["control-capture"]).to_be("orc-brain")
	-- Its model picker, as a chat's (the Brain dump's own model).
	expect(app:has(`{COMPOSER} >> role:button "Model"`) or app:has(`{COMPOSER} >> ~"Haiku"`)).to_be(true)
	expect(app:has(`{COMPOSER} >> role:button "Attach"`)).to_be(true)
	-- No access mode, no place: the Brain dump routes, it works nowhere.
	expect(app:has(`{COMPOSER} >> role:button "Access"`)).to_be(false)
	expect(app:has(`{COMPOSER} >> ~"No ref"`)).to_be(false)
end)

test("capture: Shift+Enter breaks the line, Enter sends it as a dump", function()
	local app = open()
	app:type(`{COMPOSER} >> role:textbox`, "first line")
	app:key("shift+Enter")
	app:type(`{COMPOSER} >> role:textbox`, "second")
	app:frames(3)
	expect(calls(app)).never.to_contain("dump:")
	app:key("Enter")
	app:frames(5)
	expect(calls(app)).to_contain("dump:first line\nsecond")
	expect(app:text(`{COMPOSER} >> role:textbox`)).to_be("")
end)

test("capture: a file attached goes with the dump, as a chat message's do", function()
	local app = open()
	app:copy_file("assets/favicons/zeron.png", "shot.png")
	app:pick({ "shot.png" })
	app:click(`{COMPOSER} >> role:button "Attach"`)
	app:frames(10)
	app:type(`{COMPOSER} >> role:textbox`, "look at this")
	app:key("Enter")
	app:frames(5)
	expect(calls(app)).to_contain("dump:look at this")
	-- (Named as the composer names a picture, its path kept.)
	expect(calls(app)).to_contain("dump-file:image:")
end)

test("every part says what came of it: a task's outcome too, its state as Zeron says it", function()
	local app = open()
	-- The outcome lines: the question's answer, and the context part's (done).
	expect(app:count("#control-capture >> .orch-answer")).to_be(2)
	expect(app:has('#control-capture >> .orch-answer >> ~"the tabs are gone"')).to_be(true)
	-- Each chip's mark: done, answered (still going), sent.
	expect(app:has('#control-capture >> role:button "Orchestrator screen, Context, Done"')).to_be(true)
	expect(app:has('#control-capture >> role:button "Exnomic VPN, Question, Answered"')).to_be(true)
	expect(app:has('#control-capture >> role:button "Later, Task, Delivered"')).to_be(true)
	expect(app:count("#control-capture >> .orch-chip >> .orch-route-mark") >= 4).to_be(true)
end)

test("what the person dumped with it: a picture as a thumbnail, a file as a chip", function()
	local app = open()
	expect(app:count("#control-capture >> .orch-dump-picture")).to_be(1)
	expect(app:has('#control-capture >> .orch-dump-file >> "notes.txt"')).to_be(true)
end)

test("capture takes a chat dragged from the sidebar as an @-mention", function()
	local app = mount("control_fixture.luau")
	app:resize(1440, 900)
	app:frames(30)
	app:drag('#sidebar >> role:button "Linux titlebar toggle"', `{COMPOSER} >> role:textbox`)
	app:frames(10)
	-- A chip of it on the draft, as a chat's composer has one.
	expect(app:has(`{COMPOSER} >> role:button "Remove Linux titlebar toggle"`)).to_be(true)
	-- Sent, it goes with the dump.
	app:type(`{COMPOSER} >> role:textbox`, "same fix here")
	app:key("Enter")
	app:frames(5)
	expect(tostring(app:state("calls") or "")).to_contain("dump:same fix here")
end)

test("capture's model picker shows the Brain dump's own model", function()
	local app = open()
	-- Its chat's config (Haiku), never a new chat's default (Opus, first).
	expect(app:has(`{COMPOSER} >> role:button "Model" >> "Haiku 5.5"`)).to_be(true)
end)

test("capture: a model picked is the Brain dump's at once, and goes with the dump", function()
	local app = open()
	-- (Picked in its model picker: the lane's pick, by its chat.)
	app:set_state("chat_lane_model_orc-brain", "claude-code/claude-opus-5-5")
	app:frames(10)
	-- Nothing else changed, nothing else sent.
	expect(calls(app)).never.to_contain("configure:orc-brain:,")
	expect(calls(app)).to_contain("configure:orc-brain:claude-code/claude-opus-5-5")
	app:type(`{COMPOSER} >> role:textbox`, "plan the week")
	app:key("Enter")
	app:frames(5)
	expect(calls(app)).to_contain("dump:plan the week")
	expect(calls(app)).to_contain("dump-model:claude-code/claude-opus-5-5")
end)
