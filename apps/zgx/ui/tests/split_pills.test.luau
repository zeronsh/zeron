-- The pills over a split's composers: each pane's "Jump to latest" says
-- whether that pane is scrolled up, and stands beside the pill of what waits
-- in its chat ("What's waiting": a plan, a queue) as one centred pair, never
-- on it. The focused pane's pair stands over the window's composer, every
-- other pane's over its own. One pane alone is as ever. On the shell in
-- ui/shell_fixture.luau.

local function long_chat(id: string, todos: boolean?): any
	local messages = {}
	for i = 1, 60 do
		table.insert(messages, { id = `{id}-{i}`, role = "assistant", text = string.rep(`Line {i} of {id}. `, 40) })
	end
	return {
		messages = messages,
		session = if todos then { id = id, state = "ready", todos = { { id = `{id}-t1`, text = "Read the tests", status = "in_progress" } } } else nil,
	}
end

local function leaf(id: string): any
	return { kind = "leaf", id = id }
end

-- A node's box in the window: x, y, w, h (its first line in the tree).
local function box(app: any, sel: string): (number, number, number, number)
	local line = string.match(app:tree(sel), "[^\n]+") or ""
	local x, y, w, h = string.match(line, " (%-?[%d.]+),(%-?[%d.]+) ([%d.]+)x([%d.]+)")
	expect(x ~= nil).to_be(true)
	return tonumber(x) or 0, tonumber(y) or 0, tonumber(w) or 0, tonumber(h) or 0
end

local function apart(app: any, a: string, b: string)
	local ax, ay, aw, ah = box(app, a)
	local bx, by, bw, bh = box(app, b)
	local overlap = ax < bx + bw and bx < ax + aw and ay < by + bh and by < ay + ah
	expect(overlap).to_be(false)
	-- (Side by side, on one line, a gap between.)
	expect(math.abs((ay + ah) - (by + bh)) < 2).to_be(true)
	local gap = if ax < bx then bx - (ax + aw) else ax - (bx + bw)
	expect(gap > 4 and gap < 16).to_be(true)
end

local function jump(id: string): string
	return `#lane-split-{id} >> role:button "Jump to latest"`
end

-- The window's composer's pill (the focused pane's chat's), and a pane's own.
local WINDOW_PILL = `#shell-page >> role:toolbar "What's waiting"`
local function pane_pill(id: string): string
	return `#lane-composer-split-{id} >> role:toolbar "What's waiting"`
end

local function split(app: any, chats: { [string]: any }, ids: { string })
	app:resize(1800, 820)
	app:frames(30)
	app:set_state("fixture.chats", chats)
	local children = {}
	for _, id in ids do
		table.insert(children, leaf(id))
	end
	app:set_state("inset_split_trees", { { kind = "row", children = children } })
	app:click('#shell-sidebar >> role:button "Fix the build"')
	app:frames(60)
	expect(app:state("fixture_open_chat")).to_be("a1")
end

local function scroll_up(app: any, id: string)
	app:scroll(`#lane-split-{id} >> #transcript`, -1500)
	app:frames(30)
	expect(app:wait(jump(id), { timeout_ms = 2000 })).to_be(true)
	-- (Both pills settled where they glide to.)
	app:wait('"never shown, only waited past"', { timeout_ms = 500 })
	app:frames(30)
end

test("the focused pane's Jump to latest stands beside the window composer's pill", function()
	local app = mount("shell_fixture.luau")
	split(app, { a1 = long_chat("a1", true), a2 = long_chat("a2") }, { "a1", "a2" })
	expect(app:has(WINDOW_PILL)).to_be(true)
	scroll_up(app, "a1")
	apart(app, jump("a1"), WINDOW_PILL)
	expect(app:has(jump("a2"))).to_be(false)
	expect(app:error()).to_be(nil)
end)

test("a pane that isn't focused: its Jump to latest beside its own composer's pill", function()
	local app = mount("shell_fixture.luau")
	split(app, { a1 = long_chat("a1"), a2 = long_chat("a2", true) }, { "a1", "a2" })
	expect(app:has(pane_pill("a2"))).to_be(true)
	scroll_up(app, "a2")
	apart(app, jump("a2"), pane_pill("a2"))
	expect(app:has(jump("a1"))).to_be(false)
	expect(app:error()).to_be(nil)
end)

test("three panes: only the one scrolled up offers the way back, the focus moving or not", function()
	local app = mount("shell_fixture.luau")
	split(app, { a1 = long_chat("a1"), a2 = long_chat("a2", true), a3 = long_chat("a3", true) }, { "a1", "a2", "a3" })
	scroll_up(app, "a2")
	expect(app:has(jump("a1"))).to_be(false)
	expect(app:has(jump("a3"))).to_be(false)
	-- The scrolled pane focused: the window's composer and its pill come to
	-- it, its way back beside them at once.
	app:click("#split-pane-a2")
	app:frames(30)
	expect(app:state("fixture_open_chat")).to_be("a2")
	expect(app:has(jump("a2"))).to_be(true)
	app:wait('"never shown, only waited past"', { timeout_ms = 500 })
	app:frames(30)
	apart(app, jump("a2"), WINDOW_PILL)
	-- Another focused: a2 keeps its way back, beside its own pill again; a3's
	-- pill, the window's now, stands alone in its centre.
	app:click("#split-pane-a3")
	app:frames(30)
	expect(app:state("fixture_open_chat")).to_be("a3")
	app:wait('"never shown, only waited past"', { timeout_ms = 500 })
	app:frames(30)
	expect(app:has(jump("a2"))).to_be(true)
	expect(app:has(jump("a3"))).to_be(false)
	apart(app, jump("a2"), pane_pill("a2"))
	local px, _, pw = box(app, WINDOW_PILL)
	local cx, _, cw = box(app, "#split-pane-a3")
	expect(math.abs((px + pw / 2) - (cx + cw / 2)) < 2).to_be(true)
	expect(app:error()).to_be(nil)
end)

test("one pane: Jump to latest and the composer's pill stand side by side, as ever", function()
	local app = mount("shell_fixture.luau")
	app:resize(1800, 820)
	app:frames(30)
	app:set_state("fixture.chats", { a1 = long_chat("a1", true) })
	app:click('#shell-sidebar >> role:button "Fix the build"')
	app:frames(60)
	expect(app:has(WINDOW_PILL)).to_be(true)
	local wx, _, ww = box(app, WINDOW_PILL)
	local px, _, pw = box(app, "#shell-page")
	expect(math.abs((wx + ww / 2) - (px + pw / 2)) < 2).to_be(true)
	app:scroll("#shell-page >> #transcript", -1500)
	app:frames(30)
	local page_jump = '#shell-page >> role:button "Jump to latest"'
	expect(app:wait(page_jump, { timeout_ms = 2000 })).to_be(true)
	app:wait('"never shown, only waited past"', { timeout_ms = 500 })
	app:frames(30)
	apart(app, page_jump, WINDOW_PILL)
	expect(app:error()).to_be(nil)
end)

test("a split begun with the chat page scrolled up: the pill isn't moved aside for a way back that's gone", function()
	local app = mount("shell_fixture.luau")
	app:resize(1800, 820)
	app:frames(30)
	app:set_state("fixture.chats", { a1 = long_chat("a1", true), a2 = long_chat("a2") })
	app:click('#shell-sidebar >> role:button "Fix the build"')
	app:frames(60)
	app:scroll("#shell-page >> #transcript", -1500)
	app:frames(30)
	expect(app:wait('#shell-page >> role:button "Jump to latest"', { timeout_ms = 2000 })).to_be(true)
	-- The chat page goes for the split; the pane opens at its latest.
	app:set_state("fixture.chats", { a1 = long_chat("a1", true), a2 = long_chat("a2") })
	app:set_state("inset_split_trees", { { kind = "row", children = { leaf("a1"), leaf("a2") } } })
	app:frames(60)
	app:scroll("#lane-split-a1 >> #transcript", 5000)
	app:frames(30)
	app:wait('"never shown, only waited past"', { timeout_ms = 500 })
	app:frames(30)
	expect(app:has(jump("a1"))).to_be(false)
	local wx, _, ww = box(app, WINDOW_PILL)
	local cx, _, cw = box(app, "#split-pane-a1")
	expect(math.abs((wx + ww / 2) - (cx + cw / 2)) < 2).to_be(true)
	expect(app:error()).to_be(nil)
end)

-- A send from the window's composer in a split: the focused pane's
-- transcript goes to its end, its prompt in it, as the chat page's does;
-- the other panes stay where they were read.
local function ready(id: string): any
	local c = long_chat(id)
	c.session = { id = id, state = "ready" }
	return c
end

local function send(app: any, words: string)
	app:type('#shell-page >> role:textbox "Message"', words)
	app:key("Enter")
	app:frames(60)
	app:wait('"never shown, only waited past"', { timeout_ms = 700 })
	app:frames(30)
end

test("a split: a send from the window's composer takes the focused pane to its end, the others stay", function()
	local app = mount("shell_fixture.luau")
	split(app, { a1 = ready("a1"), a2 = ready("a2") }, { "a1", "a2" })
	scroll_up(app, "a2")
	scroll_up(app, "a1")
	send(app, "Sent from the split")
	expect(app:wait(jump("a1"), { gone = true, timeout_ms = 2000 })).to_be(true)
	expect(app:has('#lane-split-a1 >> "Sent from the split"')).to_be(true)
	expect(app:has('#lane-split-a2 >> "Sent from the split"')).to_be(false)
	expect(app:has(jump("a2"))).to_be(true)
	expect(app:error()).to_be(nil)
end)

test("a split: the other pane focused, a send takes that pane alone to its end", function()
	local app = mount("shell_fixture.luau")
	split(app, { a1 = ready("a1"), a2 = ready("a2") }, { "a1", "a2" })
	scroll_up(app, "a1")
	scroll_up(app, "a2")
	app:click("#split-pane-a2")
	app:frames(30)
	expect(app:state("fixture_open_chat")).to_be("a2")
	send(app, "Sent to the second")
	expect(app:wait(jump("a2"), { gone = true, timeout_ms = 2000 })).to_be(true)
	expect(app:has('#lane-split-a2 >> "Sent to the second"')).to_be(true)
	expect(app:has('#lane-split-a1 >> "Sent to the second"')).to_be(false)
	expect(app:has(jump("a1"))).to_be(true)
	expect(app:error()).to_be(nil)
end)
