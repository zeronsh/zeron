-- A side chat does what the chat page does, as far as the two can: what `@`,
-- `/` and `$` offer, ⌘F and ⌘/, Escape and what sends, its Fast and plan mode,
-- Retry, a passage quoted, its device gone. And its tab reads as a tab on the
-- top bar: its project's tile, its live name, what it's at, every inset even.
-- On the page in ui/lanes_fixture.luau.

local BOX = ".side-chat-tab >> role:textbox"

local function open(): any
	local app = mount("lanes_fixture.luau")
	app:resize(1200, 900)
	app:frames(30)
	app:click('#session-lanes >> role:button "Open side chat Looking at the tests"')
	app:frames(30)
	return app
end

local function calls(app: any): string
	return tostring(app:state("calls") or "")
end

-- A node's box in the window, from its tree line: x, y, w, h.
local function box(app: any, line_has: string, within: string): (number, number, number, number)
	for line in app:tree(within):gmatch("[^\n]+") do
		if line:find(line_has, 1, true) then
			local x, y, w, h = line:match(" (%-?[%d%.]+),(%-?[%d%.]+) ([%d%.]+)x([%d%.]+)")
			return tonumber(x) :: number, tonumber(y) :: number, tonumber(w) :: number, tonumber(h) :: number
		end
	end
	error(`no "{line_has}" in {app:tree(within)}`)
end

-- Its tab ------------------------------------------------------------------

test("a side chat's tab shows its project's tile, not the side chat's glyph", function()
	local app = open()
	-- (Its project's, "atlas": its first letter, on its hue.)
	expect(app:tree("#panel-tab-tab1")).to_contain('"A"')
	app:command("side_no_project")
	app:frames(10)
	expect(app:tree("#panel-tab-tab1")).never.to_contain('"A"')
end)

test("a new side chat's tile is its parent's project until it's moved", function()
	local app = mount("lanes_fixture.luau")
	app:resize(1200, 900)
	app:frames(30)
	app:click('#session-lanes >> role:button "New side chat"')
	app:frames(30)
	expect(app:tree("#panel-tab-tab1")).to_contain('"A"')
end)

test("a side chat's tab follows its chat's name", function()
	local app = open()
	expect(app:tree("#panel-strip")).to_contain("Looking at the tests")
	app:command("side_renamed")
	app:frames(10)
	expect(app:tree("#panel-strip")).to_contain("Renamed by a person")
end)

test("a side chat's tab says what it's at, × under the pointer", function()
	local app = open()
	expect(app:has("#panel-tab-tab1-state")).to_be(true)
	app:hover("#panel-tab-tab1")
	app:frames(10)
	expect(app:has("#panel-tab-tab1-state")).to_be(false)
	expect(app:has('#panel-tab-tab1 >> role:button "Close Tab"')).to_be(true)
end)

test("a panel tab's tile and × stand as far in from its ends as from its top and bottom", function()
	local app = open()
	app:hover("#panel-tab-tab1")
	app:frames(10)
	local tx, ty, tw, th = box(app, "#panel-tab-tab1", "#panel-tab-tab1")
	local cx, cy, cw, ch = box(app, 'button "Close Tab"', "#panel-tab-tab1")
	expect(cw).to_be(20)
	expect(ch).to_be(20)
	expect(cy - ty).to_be(4)
	expect(ty + th - (cy + ch)).to_be(4)
	expect(tx + tw - (cx + cw)).to_be(4)
end)

-- Its completions ----------------------------------------------------------

test("@ in a side chat offers the files of where it works", function()
	local app = open()
	app:type(BOX, "@lib")
	app:frames(30)
	app:wait('"lib.rs"', { timeout_ms = 1000 })
	expect(calls(app)).to_contain("search:side-a:lib")
end)

test("@ in a new side chat searches its project", function()
	local app = mount("lanes_fixture.luau")
	app:resize(1200, 900)
	app:frames(30)
	app:click('#session-lanes >> role:button "New side chat"')
	app:frames(30)
	app:type(BOX, "@lib")
	app:wait('"lib.rs"', { timeout_ms = 1000 })
	expect(calls(app)).to_contain("search:p1:lib")
end)

test("/ in a side chat offers the agent's commands, then Zeron's", function()
	local app = open()
	app:type(BOX, "/")
	app:frames(30)
	expect(calls(app)).to_contain("invocations:codex:side-a")
	expect(app:has('"/review"')).to_be(true)
	expect(app:has('"/stop"')).to_be(true)
end)

test("a draft that's only /stop stops this side chat and sends nothing", function()
	local app = open()
	app:command("side_running")
	app:frames(5)
	app:type(BOX, "/stop")
	app:frames(30)
	app:key("Enter")
	app:frames(20)
	expect(calls(app)).to_contain("stop:side-a")
	expect(calls(app)).never.to_contain("send:side-a")
end)

-- Its keys -----------------------------------------------------------------

test("a new side chat's composer takes the keys", function()
	local app = mount("lanes_fixture.luau")
	app:resize(1200, 900)
	app:frames(30)
	app:click('#session-lanes >> role:button "New side chat"')
	app:frames(30)
	expect(app:tree(BOX)).to_contain("focused")
end)

test("Enter follows the setting that says what sends", function()
	local app = open()
	app:command("send_mod_enter")
	app:type(BOX, "line one")
	app:key("Enter")
	app:frames(10)
	expect(calls(app)).never.to_contain("send:side-a")
	app:key("cmd+enter")
	app:frames(10)
	expect(calls(app)).to_contain("send:side-a")
end)

test("Escape stops a working side chat under the setting, not the chat page's", function()
	local app = open()
	app:command("escape_stops")
	app:command("side_running")
	app:frames(5)
	app:click(BOX)
	app:frames(5)
	app:key("escape")
	app:frames(10)
	expect(calls(app)).to_contain("stop:side-a")
end)

test("⌘F in a side chat finds in its own transcript", function()
	local app = open()
	app:click(BOX)
	app:frames(5)
	app:command("lane_find")
	app:frames(10)
	expect(calls(app)).to_contain("find:true")
	expect(app:has("#lane-find-tab1")).to_be(true)
	expect(app:has("#chat-find")).to_be(false)
	app:type('#lane-find-tab1 >> role:textbox "Find"', "side-a")
	app:frames(10)
	expect(app:tree("#lane-find-tab1")).to_contain("1 of 1")
	app:key("escape")
	app:frames(5)
	expect(app:has("#lane-find-tab1")).to_be(false)
end)

test("⌘F and ⌘/ leave a panel without the keyboard to the chat page", function()
	local app = open()
	app:command("lane_find")
	app:command("lane_model")
	app:frames(5)
	expect(calls(app)).to_contain("find:false")
	expect(calls(app)).to_contain("model:false")
end)

test("⌘/ in a side chat opens its own model picker", function()
	local app = open()
	app:click(BOX)
	app:frames(5)
	app:command("lane_model")
	app:frames(30)
	expect(calls(app)).to_contain("model:true")
	expect(app:has('.side-chat-tab >> role:textbox "Search"')).to_be(true)
end)

-- What it sends ------------------------------------------------------------

test("a side chat's Fast and plan mode go with its message", function()
	local app = open()
	app:command("side_fast")
	app:frames(10)
	app:type(BOX, "go")
	app:key("Enter")
	app:frames(10)
	expect(calls(app)).to_contain("mode:side-a:fast:plan")
end)

test("Retry on a message that didn't go retries it in this side chat", function()
	local app = open()
	app:command("side_failed")
	app:frames(20)
	app:click('.side-chat-tab >> role:button "Not delivered — send again"')
	app:frames(10)
	expect(calls(app)).to_contain("retry:side-a:lost1")
end)

test("a passage attached to a side chat goes with its message, in full", function()
	local app = open()
	app:command("side_attach_passage")
	app:command("side_attach_passage")
	app:frames(10)
	expect(app:count('#lane-composer-tab1 >> ~"Reply"')).to_be(1)
	app:type(BOX, "why this?")
	app:key("Enter")
	app:frames(10)
	expect(calls(app)).to_contain("the whole passage")
end)

test("a passage mentioned in a side chat's draft goes in full, and comes back with it if it doesn't go", function()
	local app = open()
	local line = "> the whole…"
	app:set_state("chat_lane_draft_side-a", line)
	app:set_state("chat_lane_quotes_side-a", { [line] = "the whole passage" })
	app:frames(5)
	app:command("send_fails")
	app:click(BOX)
	app:key("Enter")
	app:frames(5)
	expect(calls(app)).to_contain("the whole passage")
	app:frames(30)
	expect(app:text(BOX)).to_be(line)
end)

test("a tab made again after its edit was let go puts its own draft back", function()
	local app = open()
	app:type(BOX, "My own draft")
	app:command("side_queue_one")
	app:frames(20)
	-- (The queue waits in the pill, folded, until opened.)
	app:click('.side-chat-tab >> role:button "Open Queued"')
	app:frames(20)
	app:hover('.side-chat-tab >> role:listitem "1. Queued words"')
	app:frames(10)
	app:click('.side-chat-tab >> role:button "Edit"')
	app:frames(10)
	expect(app:text(BOX)).to_be("Queued words")
	app:command("side_edit_lost")
	app:frames(10)
	expect(app:text(BOX)).to_be("My own draft")
end)

test("its device gone, a side chat's composer is disabled", function()
	local app = open()
	app:command("side_offline")
	app:frames(10)
	local attach = '.side-chat-tab >> role:button "Attach"'
	expect(app:tree(attach)).to_contain("disabled")
end)
