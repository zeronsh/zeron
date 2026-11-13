-- The shell from the keys alone: Tab reaches what's there to use, in the
-- order it reads (title bar, sidebar from its head, the page, the panel),
-- and nothing hidden. On the shell in ui/shell_fixture.luau.

local function open(): any
	local app = mount("shell_fixture.luau")
	app:resize(1200, 800)
	app:frames(30)
	return app
end

-- The focused node's line, as the tree says. (The window's dump stops
-- past a budget: the title bar, drawn last, is read on its own too.)
local function focused(app: any): string
	for _, dump in { app:tree(), app:tree("#titlebar") } do
		for line in dump:gmatch("[^\n]+") do
			if line:find(" focused", 1, true) then
				return line
			end
		end
	end
	return ""
end

-- Tab `n` times from where focus is: each stop's line, in order.
local function walk(app: any, n: number): { string }
	local stops = {}
	for _ = 1, n do
		app:key("Tab")
		app:frames(4)
		table.insert(stops, focused(app))
	end
	return stops
end

local function find(stops: { string }, what: string): number?
	for i, line in stops do
		if line:find(what, 1, true) then
			return i
		end
	end
	return nil
end

test("the panel's edge is a Tab stop only while the panel shows", function()
	local app = open()
	app:click('#shell-sidebar >> role:button "Fix the build"')
	app:frames(30)
	local closed = walk(app, 60)
	expect(find(closed, "Resize files")).to_be_nil()
	app:click('#titlebar >> role:button "Panel"')
	app:frames(60)
	local opened = walk(app, 80)
	expect(find(opened, "Resize files")).never.to_be_nil()
end)

test("Tab reads each part of the shell in its order: the sidebar's rows, the page, its panel, the title bar left to right", function()
	-- (The title bar, the sidebar's head and the panel's edge are layers
	-- over what's under them: they stay where they paint, last, so a press
	-- reaches what it's over; zgx has no tab order of its own to give.)
	local app = open()
	app:click('#shell-sidebar >> role:button "Fix the build"')
	app:frames(30)
	app:click('#titlebar >> role:button "Panel"')
	app:frames(60)
	local stops = walk(app, 90)
	local from = find(stops, '"New chat"') :: number
	local round = table.move(stops, from, #stops, 1, {})
	local function at(what: string): number
		local i = find(round, what)
		if not i then
			error(`no stop at {what}:\n{table.concat(round, "\n")}`)
		end
		return i :: number
	end
	local order = {
		'"New chat"',
		'"Fix the build"',
		'"Local"',
		'textbox "Message"',
		'"Browser"',
		'"Resize files"',
		'"Toggle sidebar"',
		'"Back"',
		'"Panel"',
	}
	for i = 2, #order do
		if at(order[i - 1]) >= at(order[i]) then
			error(`{order[i - 1]} should come before {order[i]}:\n{table.concat(round, "\n")}`)
		end
	end
end)
test("I with no text field holding the keyboard goes into the composer; Escape there gives the keyboard back", function()
	local app = open()
	app:click('#shell-sidebar >> role:button "Fix the build"')
	app:frames(30)
	app:click("role:textbox")
	app:frames(5)
	expect(focused(app)).to_contain("textbox")
	app:key("Escape")
	app:frames(5)
	expect(focused(app)).never.to_contain("textbox")
	app:key("i")
	app:frames(5)
	expect(focused(app)).to_contain("textbox")
	-- Typed into the composer, I is a letter of the draft.
	app:key("i")
	app:frames(5)
	expect(focused(app)).to_contain("textbox")
end)
test("⌘F with nothing focused taking it opens a session's find bar, and nothing on a new chat", function()
	local app = open()
	-- (The bar draws over a transcript, which this fixture's chat has none
	-- of: whether it's open is what's asked.)
	app:command("shell.find")
	app:frames(20)
	expect(app:state("chat.find.shown") == true).to_be(false)
	app:click('#shell-sidebar >> role:button "Fix the build"')
	app:frames(30)
	app:command("shell.find")
	app:frames(20)
	expect(app:state("chat.find.shown")).to_be(true)
end)
