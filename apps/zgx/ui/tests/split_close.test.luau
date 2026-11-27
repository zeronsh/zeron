-- A split's pane has a way out you can see: an × at its top right while the
-- pointer is over it, taking it out of the split as ⌘W does the focused one.
-- Its chat goes on, in the sidebar. On the shell in ui/shell_fixture.luau.

local function leaf(id: string): any
	return { kind = "leaf", id = id }
end

local function chat(id: string): any
	return { messages = { { id = `{id}-1`, role = "assistant", text = `Hello from {id}.` } } }
end

local function close_button(id: string): string
	return `#split-pane-{id} >> role:button "Remove from split"`
end

local function split(app: any, ids: { string })
	app:resize(1800, 820)
	app:frames(30)
	local chats = {}
	local children = {}
	for _, id in ids do
		chats[id] = chat(id)
		table.insert(children, leaf(id))
	end
	app:set_state("fixture.chats", chats)
	app:set_state("inset_split_trees", { { kind = "row", children = children } })
	app:click('#shell-sidebar >> role:button "Fix the build"')
	app:frames(60)
	expect(app:state("fixture_open_chat")).to_be(ids[1])
end

test("a pane's × shows on hover and takes a pane that isn't focused out", function()
	local app = mount("shell_fixture.luau")
	split(app, { "a1", "a2", "a3" })
	expect(app:has(close_button("a2"))).to_be(false)
	app:hover("#split-pane-a2")
	app:frames(10)
	expect(app:has(close_button("a2"))).to_be(true)
	app:click(close_button("a2"))
	app:frames(60)
	expect(app:has("#split-pane-a2")).to_be(false)
	expect(app:state("fixture_open_chat")).to_be("a1")
	expect(app:has("#split-pane-a1")).to_be(true)
	expect(app:has("#split-pane-a3")).to_be(true)
	expect(app:error()).to_be(nil)
end)

test("the focused pane's × leaves its neighbour alone, no split, and focused", function()
	local app = mount("shell_fixture.luau")
	split(app, { "a1", "a2" })
	app:hover("#split-pane-a1")
	app:frames(10)
	app:click(close_button("a1"))
	app:frames(60)
	-- (As ⌘W leaves it: a2 alone, a tab of one, no split.)
	local trees = app:state("inset_split_trees") :: any
	expect(#trees).to_be(1)
	expect(trees[1].kind).to_be("leaf")
	expect(trees[1].id).to_be("a2")
	expect(app:has(close_button("a2"))).to_be(false)
	expect(app:state("fixture_open_chat")).to_be("a2")
	expect(app:error()).to_be(nil)
end)
