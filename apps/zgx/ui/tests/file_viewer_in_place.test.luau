-- The file viewer's header follows the open file in place: another file
-- opened keeps the header's nodes, the folders it shares with the last
-- where they were, and shows the new file's name, folders and status.
-- (The header was built again for every file, keyed by its path.) On the
-- viewers in ui/viewer_fixture.luau, one per header shape.

-- A node's identity, by its kind and label (or text) under `scope`: `@id`.
local function node(app: any, scope: string, kind: string?, label: string): string?
	local pattern = if kind then ` {kind} "{label}"` else ` "{label}"`
	for line in app:tree(scope):gmatch("[^\n]+") do
		if line:find(pattern, 1, true) then
			return line:match("@(%d+)")
		end
	end
	return nil
end

local function settle(app: any)
	app:frames(30)
end

local function open(app: any, path: string, status: string?)
	app:set_state("test_viewer_doc", { path = path, state = "ready", text = "x\n", status = status })
	settle(app)
end

test("another file keeps the header, its shared folders in place", function()
	local app = mount("viewer_fixture.luau")
	app:resize(700, 400)
	settle(app)
	local root = node(app, "#line", "button", "Show zeron in the tree")
	local src = node(app, "#line", "button", "Show src in the tree")
	local ds = node(app, "#line", "button", "Show ds in the tree")
	local name = node(app, "#line", "text", "button.luau")
	local stacked = node(app, "#stack", "button", "Show src in the tree")
	expect(src).never.to_be_nil()
	expect(name).never.to_be_nil()

	-- A file beside it, then one a folder deeper: the shared crumbs stay.
	open(app, "src/ds/card.luau", "added")
	expect(`name @{node(app, "#line", "text", "card.luau")}`).to_be(`name @{name}`)
	expect(`src @{node(app, "#line", "button", "Show src in the tree")}`).to_be(`src @{src}`)
	expect(`stacked src @{node(app, "#stack", "button", "Show src in the tree")}`).to_be(`stacked src @{stacked}`)
	expect(node(app, "#stack", "text", "Added")).never.to_be_nil()
	open(app, "src/ds/parts/card.luau", nil)
	expect(`zeron @{node(app, "#line", "button", "Show zeron in the tree")}`).to_be(`zeron @{root}`)
	expect(`src @{node(app, "#line", "button", "Show src in the tree")}`).to_be(`src @{src}`)
	expect(`ds @{node(app, "#line", "button", "Show ds in the tree")}`).to_be(`ds @{ds}`)
	expect(node(app, "#line", "button", "Show parts in the tree")).never.to_be_nil()
	expect(node(app, "#stack", "text", "Added")).to_be_nil()
	expect(app:has('#path >> "zeron/src/ds/parts/card.luau"')).to_be(true)
end)
