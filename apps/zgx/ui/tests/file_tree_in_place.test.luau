-- The file tree's stack and note update in place: a folder joining the
-- folders kept at the top as the list scrolls leaves the one already there
-- the same node, and the note at the foot keeps its node as more of a name
-- is typed. (Each was built again for every change: the stack keyed by
-- every folder in it, the note by its words, so each letter typed faded
-- the note out and in again.) On the tree in ui/tree_fixture.luau.

-- A node's identity, by its kind and label: `@id`.
local function node(app: any, kind: string, label: string): string?
	for line in app:tree():gmatch("[^\n]+") do
		if line:find(` {kind} "{label}"`, 1, true) then
			return line:match("@(%d+)")
		end
	end
	return nil
end

local function settle(app: any)
	app:frames(30)
end

test("a folder joining the stack leaves the one there the same node", function()
	local app = mount("tree_fixture.luau")
	app:resize(400, 500)
	settle(app)
	-- Past beta's rows: alpha alone at the top.
	app:set_state("test_tree_offset", 900)
	settle(app)
	local alpha = node(app, "button", "Go to alpha")
	expect(alpha).never.to_be_nil()
	expect(node(app, "button", "Go to beta")).to_be_nil()
	-- Back into beta's rows: beta joins it.
	app:set_state("test_tree_offset", 30)
	settle(app)
	expect(node(app, "button", "Go to beta")).never.to_be_nil()
	expect(`alpha @{node(app, "button", "Go to alpha")}`).to_be(`alpha @{alpha}`)
	-- And leaves again.
	app:set_state("test_tree_offset", 900)
	settle(app)
	expect(node(app, "button", "Go to beta")).to_be_nil()
	expect(`alpha @{node(app, "button", "Go to alpha")}`).to_be(`alpha @{alpha}`)
end)

test("the note keeps its node as a name is typed", function()
	local app = mount("tree_fixture.luau")
	app:resize(400, 500)
	app:set_state("test_tree_offset", 0)
	settle(app)
	app:click('role:treeitem "b05.txt"')
	app:frames(5)
	app:key("b")
	app:frames(2)
	local first = node(app, "text", "b")
	expect(first).never.to_be_nil()
	app:key("1")
	app:frames(2)
	expect(`note @{node(app, "text", "b1")}`).to_be(`note @{first}`)
end)
