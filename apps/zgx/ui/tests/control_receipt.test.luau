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
	-- Its text, whole.
	expect(app:text("#control-capture >> role:document")).to_contain("is the engine state thread still on the registry part?")
	expect(app:has('#control-capture >> role:button "Orchestrator screen, Context, Delivered"')).to_be(true)
	expect(app:has('#control-capture >> role:button "Inbox, Task, Delivered"')).to_be(true)
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
	expect(app:count("#control-capture >> .orch-answer")).to_be(1)
	app:click('#control-capture >> .orch-answer-open')
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

test("each chip's menu opens under it, from a box of its own, with places to move it", function()
	local app = open()
	-- An odd width: the column centred, a chip starts between points.
	app:resize(2023, 1102)
	app:frames(30)
	local n = app:count("#control-capture >> .orch-chip")
	expect(n).to_be(5)
	for i = 1, n do
		local chip = `#control-capture >> .orch-chip [{i}]`
		local name = string.match(app:tree(chip), 'button "([^"]*)"') or ""
		local _, cy, _, ch = box_of(app, `button "{name}"`)
		app:click(chip)
		app:frames(5)
		expect(app:error()).to_be_nil()
		local items = menu_items(app)
		expect(#items > 0).to_be(true)
		expect(table.concat(items, ",")).to_contain("New thread")
		-- Its panel a size, 4 under the chip.
		local _, my, mw, mh = box_of(app, ' menu "')
		expect(mw > 0 and mh > 0).to_be(true)
		expect(my).to_be(cy + ch + 4)
		app:key("Escape")
		app:frames(5)
	end
	-- An unsure part's chip too.
	app:click("#control-history-toggle")
	app:frames(10)
	app:click(".control-history-row")
	app:frames(20)
	app:click('#control-capture >> role:button "Unsure, Task, Unsure"')
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
