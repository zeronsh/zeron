-- The sidebar's "Nest side chats" (Show): off, side chats aren't listed;
-- on, each chat's side chats stand under it a step in (newest first, as
-- the lanes list them) and fold away with a chevron on the chat, a subagent's
-- chat not among them; the ones in a state stay while it's folded ("Active
-- when collapsed"), and a side chat whose chat isn't listed is a row of its
-- own in its project. On the shell in ui/shell_fixture.luau.

local SIDE = "#shell-sidebar"
local K = "screens/shell/sidebar.luau::"
local NEST = `{K}shell_sidebar_side_chats`
local KEEP = `{K}shell_sidebar_keep_active`

local function open(): any
	local app = mount("shell_fixture.luau")
	app:resize(1200, 900)
	app:frames(30)
	app:command("side_chat_kinds")
	app:frames(10)
	return app
end

local function shows(app: any, label: string): boolean
	return app:has(`{SIDE} >> role:button "{label}"`)
end

-- Where a row stands (its top), and where its first glyph or letter does
-- (how far in it starts), by its label.
local function top(app: any, label: string): number
	for line in app:tree(SIDE):gmatch("[^\n]+") do
		if line:find(` button "{label}"`, 1, true) then
			return tonumber(line:match(" %-?%d+,(%-?%d+) ")) :: number
		end
	end
	error(`no "{label}" in the sidebar`)
end
local function lead(app: any, label: string): number
	local found = false
	for line in app:tree(`{SIDE} >> role:button "{label}"`):gmatch("[^\n]+") do
		local x = line:match(" (%-?%d+),%-?%d+ %d+x%d+")
		if found and x and (line:find(" img ", 1, true) or line:find(' text "', 1, true)) then
			return tonumber(x) :: number
		end
		found = true
	end
	error(`no mark on "{label}"`)
end

test("side chats aren't listed until they're nested", function()
	local app = open()
	expect(app:state("shell_sidebar_side_chats")).to_be(false)
	expect(shows(app, "Fix the build")).to_be(true)
	expect(shows(app, "Look into the flake")).to_be(false)
	expect(shows(app, "Left behind")).to_be(false)
	expect(app:has(`{SIDE} >> role:button "Fold side chats"`)).to_be(false)

	-- The view options offer it, and choosing it nests them.
	app:click(`{SIDE} >> role:button "Sidebar view options"`)
	app:frames(20)
	expect(app:has('"Nest side chats"')).to_be(true)
	app:click('"Nest side chats"')
	app:frames(30)
	expect(shows(app, "Look into the flake")).to_be(true)
end)

test("nested, they stand under their chat a step in, newest first", function()
	local app = open()
	app:set_state(NEST, true)
	app:frames(40)
	local fix, flake, log = top(app, "Fix the build"), top(app, "Look into the flake"), top(app, "Write the changelog")
	expect(fix < flake and flake < log and log < top(app, "Write the docs")).to_be(true)
	expect(lead(app, "Look into the flake") > lead(app, "Fix the build")).to_be(true)
	expect(lead(app, "Write the changelog")).to_be(lead(app, "Look into the flake"))
	-- (Not a side chat: a subagent's chat.)
	expect(shows(app, "Run the linter")).to_be(false)
	-- Another chat's row holds its place and its own indent.
	expect(lead(app, "Write the docs")).to_be(lead(app, "Fix the build"))
	-- Off again: gone from the list.
	app:set_state(NEST, false)
	app:frames(40)
	expect(shows(app, "Look into the flake")).to_be(false)
end)

test("one without its chat in the list is a row of its own", function()
	local app = open()
	app:set_state(NEST, true)
	app:frames(40)
	expect(shows(app, "Left behind")).to_be(true)
	-- As a chat of Atlas's is, not a step in.
	expect(lead(app, "Left behind")).to_be(lead(app, "Write the docs"))
	expect(top(app, "Left behind") > top(app, "Tidy the tests") - 1).to_be(true)
end)

test("folded, a side chat in a state stays until it settles, and all but it go", function()
	local app = open()
	app:set_state(NEST, true)
	app:frames(40)
	app:click(`{SIDE} >> role:button "Fold side chats"`)
	app:frames(40)
	-- Looking into the flake is working: it stays (Active when collapsed).
	expect(shows(app, "Look into the flake")).to_be(true)
	expect(shows(app, "Write the changelog")).to_be(false)
	expect(app:has(`{SIDE} >> role:button "Show side chats"`)).to_be(true)
	-- Not kept: it goes with the rest.
	app:set_state(KEEP, false)
	app:frames(40)
	expect(shows(app, "Look into the flake")).to_be(false)
	expect(shows(app, "Fix the build")).to_be(true)
	-- Unfolded: all of them.
	app:click(`{SIDE} >> role:button "Show side chats"`)
	app:frames(40)
	expect(shows(app, "Look into the flake")).to_be(true)
	expect(shows(app, "Write the changelog")).to_be(true)
end)

test("the side chats working show on their chat's row folded away", function()
	local app = open()
	app:set_state(NEST, true)
	app:frames(40)
	app:click(`{SIDE} >> role:button "Fold side chats"`)
	app:set_state(KEEP, false)
	app:frames(40)
	expect(shows(app, "Look into the flake")).to_be(false)
	-- (The chat's own working state, and its pill.)
	expect(app:has(`{SIDE} >> role:button "Fix the build" >> "1 subagent running"`) or app:has(`{SIDE} >> "1 subagent running"`) or app:has(`{SIDE} >> "2 subagents running"`)).to_be(true)
end)
