-- A split pane's × comes only for its head: unseen at rest, there while the
-- pointer is over the head or the keyboard is on it. On the shell in
-- ui/shell_fixture.luau, two chats side by side.

local CLOSE = '#split-pane-a2 >> role:button "Close pane"'

local function open_split(): any
	local app = mount("shell_fixture.luau")
	app:resize(1400, 800)
	app:frames(30)
	app:set_state("inset_split_trees", { { kind = "row", children = { { kind = "leaf", id = "a1" }, { kind = "leaf", id = "a2" } } } })
	app:click('#shell-sidebar >> role:button "Fix the build"')
	expect(app:wait(CLOSE, { timeout_ms = 15000 })).to_be(true)
	app:frames(30)
	expect(app:error()).to_be_nil()
	return app
end

-- The line of the x's glyph in chat `id`'s pane: "hidden" while unseen.
local function glyph_line(app: any, id: string): string
	local past = false
	for line in app:tree(`#split-pane-{id}`):gmatch("[^\n]+") do
		if past then
			return line
		end
		past = line:find('"Close pane"', 1, true) ~= nil
	end
	error("no x")
end

local function shown(app: any, id: string): boolean
	return not glyph_line(app, id):find(" hidden", 1, true)
end

-- The pane's head: the button named for its chat.
local A1_HEAD = '#split-pane-a1 >> role:button "Fix the build"'
local A2_HEAD = '#split-pane-a2 >> role:button "Write the docs"'
-- Somewhere that's no head's: the sidebar's first row.
local AWAY = '#shell-sidebar >> role:button "Tidy the tests"'

local function eventually(app: any, ok: () -> boolean): boolean
	for _ = 1, 100 do
		if ok() then
			return true
		end
		app:wait('"never shown, only waited past"', { timeout_ms = 50 })
	end
	return ok()
end

test("at rest neither pane's x shows, focused or not", function()
	local app = open_split()
	expect(shown(app, "a1")).to_be(false)
	expect(shown(app, "a2")).to_be(false)
end)

test("the pointer over a head shows that pane's x alone, and leaving hides it, focused or not", function()
	local app = open_split()
	app:hover(A2_HEAD)
	expect(eventually(app, function(): boolean
		return shown(app, "a2")
	end)).to_be(true)
	expect(shown(app, "a1")).to_be(false)
	-- (Onto the x itself, it stays: it's in the head.)
	app:hover(CLOSE)
	app:frames(30)
	expect(shown(app, "a2")).to_be(true)
	app:hover(A1_HEAD)
	expect(eventually(app, function(): boolean
		return shown(app, "a1") and not shown(app, "a2")
	end)).to_be(true)
	app:hover(AWAY)
	expect(eventually(app, function(): boolean
		return not shown(app, "a1")
	end)).to_be(true)
	expect(app:error()).to_be_nil()
end)

test("keyboard focus reaches the x and shows it; moving on hides it", function()
	local app = open_split()
	-- (The focused pane's comes first: Tab goes on into the other's chat,
	-- which focuses that pane.)
	local function on_x(): boolean
		local line = app:tree("#split-pane-a1"):match('[^\n]*"Close pane"[^\n]*')
		return line ~= nil and line:find(" focused", 1, true) ~= nil
	end
	local reached = false
	for _ = 1, 30 do
		app:key("Tab")
		app:frames(4)
		if on_x() then
			reached = true
			break
		end
	end
	expect(reached).to_be(true)
	expect(eventually(app, function(): boolean
		return shown(app, "a1")
	end)).to_be(true)
	app:key("Tab")
	expect(eventually(app, function(): boolean
		return not on_x() and not shown(app, "a1")
	end)).to_be(true)
	expect(app:error()).to_be_nil()
end)

test("shown, the x closes its pane", function()
	local app = open_split()
	app:hover(A2_HEAD)
	expect(eventually(app, function(): boolean
		return shown(app, "a2")
	end)).to_be(true)
	app:click(CLOSE)
	expect(eventually(app, function(): boolean
		return not app:has("#split-pane-a2")
	end)).to_be(true)
	expect(app:error()).to_be_nil()
end)

test("a press where the unseen x stands is the head's: its pane takes focus and stays", function()
	local app = open_split()
	-- (The pointer arrives with the press: nothing has shown the x.)
	expect(shown(app, "a2")).to_be(false)
	expect(tostring(app:state("shell_route_path")):find("a1", 1, true) ~= nil).to_be(true)
	app:click(CLOSE)
	expect(eventually(app, function(): boolean
		return tostring(app:state("shell_route_path")):find("a2", 1, true) ~= nil
	end)).to_be(true)
	expect(app:has("#split-pane-a2")).to_be(true)
	expect(app:error()).to_be_nil()
end)

test("nothing in the head moves as the x comes", function()
	local app = open_split()
	local function where(name: string): string
		return app:tree("#split-pane-a2"):match(`{name}" ([%d%.,]+ [%d%.x]+)`) or ""
	end
	local head, x = where("Write the docs"), where("Close pane")
	expect(head ~= "" and x ~= "").to_be(true)
	app:hover(A2_HEAD)
	app:frames(30)
	expect(shown(app, "a2")).to_be(true)
	expect(where("Write the docs")).to_be(head)
	expect(where("Close pane")).to_be(x)
end)

test("the x fades as the pointer leaves", function()
	local app = open_split()
	app:hover(A2_HEAD)
	app:frames(60)
	app:hover(AWAY)
	app:frames(3)
	expect(shown(app, "a2")).to_be(true)
	expect(eventually(app, function(): boolean
		return not shown(app, "a2")
	end)).to_be(true)
end)

test("with motion reduced the x goes at once", function()
	local app = mount("shell_fixture.luau")
	app:resize(1400, 800)
	app:frames(30)
	app:set_state("motion_reduce", true)
	app:set_state("inset_split_trees", { { kind = "row", children = { { kind = "leaf", id = "a1" }, { kind = "leaf", id = "a2" } } } })
	app:click('#shell-sidebar >> role:button "Fix the build"')
	expect(app:wait(CLOSE, { timeout_ms = 15000 })).to_be(true)
	app:frames(30)
	app:hover(A2_HEAD)
	app:frames(3)
	expect(shown(app, "a2")).to_be(true)
	app:hover(AWAY)
	app:frames(3)
	expect(shown(app, "a2")).to_be(false)
	expect(app:error()).to_be_nil()
end)
