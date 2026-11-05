-- "Jump to latest" in a split's pane: the pill has the keys as it's clicked and
-- leaves as the view arrives. The focus stays on that pane's chat, not on the
-- next control's, another pane's composer. On the shell in ui/shell_fixture.luau.

local function long_chat(id: string): { any }
	local messages = {}
	for i = 1, 60 do
		table.insert(messages, { id = `{id}-{i}`, role = "assistant", text = string.rep(`Line {i} of {id}. `, 40) })
	end
	return messages
end

local function leaf(id: string): any
	return { kind = "leaf", id = id }
end

local function scrolled_up(app: any, id: string)
	app:scroll(`#lane-split-{id} >> #transcript`, -1500)
	app:frames(30)
	expect(app:wait(`#lane-split-{id} >> role:button "Jump to latest"`, { timeout_ms = 2000 })).to_be(true)
end

test("Jump to latest keeps the focus on the pane it was pressed in", function()
	local app = mount("shell_fixture.luau")
	app:resize(1500, 800)
	app:frames(30)
	app:set_state("fixture.chats", { a1 = { messages = long_chat("a1") }, a2 = { messages = long_chat("a2") } })
	app:set_state("inset_split_trees", { { kind = "row", children = { leaf("a1"), leaf("a2") } } })
	app:click('#shell-sidebar >> role:button "Fix the build"')
	app:frames(60)
	expect(app:state("fixture_open_chat")).to_be("a1")
	scrolled_up(app, "a1")
	app:click('#lane-split-a1 >> role:button "Jump to latest"')
	expect(app:wait('#lane-split-a1 >> role:button "Jump to latest"', { gone = true, timeout_ms = 2000 })).to_be(true)
	app:frames(30)
	expect(app:state("fixture_open_chat")).to_be("a1")
end)

test("Jump to latest in a pane that wasn't focused focuses that pane", function()
	local app = mount("shell_fixture.luau")
	app:resize(1500, 800)
	app:frames(30)
	app:set_state("fixture.chats", { a1 = { messages = long_chat("a1") }, a2 = { messages = long_chat("a2") } })
	app:set_state("inset_split_trees", { { kind = "row", children = { leaf("a1"), leaf("a2") } } })
	app:click('#shell-sidebar >> role:button "Fix the build"')
	app:frames(60)
	scrolled_up(app, "a2")
	app:click('#lane-split-a2 >> role:button "Jump to latest"')
	expect(app:wait('#lane-split-a2 >> role:button "Jump to latest"', { gone = true, timeout_ms = 2000 })).to_be(true)
	app:frames(30)
	expect(app:state("fixture_open_chat")).to_be("a2")
end)
