-- A faded scroll view's scrollbar stands over it, not in it: a view's edge
-- `fade` masks everything inside it, the engine's own bar included (zgx
-- mounts the bar as the view's child), so a faded view's thumb dissolved
-- at its ends with the rows. The app's bar is the rail (ds/rail), the
-- view's sibling: no bar inside the view, the rail beside it along its
-- whole height. On the file tree in ui/tree_fixture.luau.

-- Each line of the tree, with its depth: `{ depth, line }`.
local function lines(app: any): { { depth: number, line: string } }
	local out = {}
	for line in app:tree():gmatch("[^\n]+") do
		local indent = line:match("^( *)@") or ""
		table.insert(out, { depth = #indent // 2, line = line })
	end
	return out
end

local function box(line: string): { number }
	local x, y, w, h = line:match(" (%-?%d+),(%-?%d+) (%d+)x(%d+)")
	return { tonumber(x) :: number, tonumber(y) :: number, tonumber(w) :: number, tonumber(h) :: number }
end

test("the file tree's scrollbar is over its faded list, not in it", function()
	local app = mount("tree_fixture.luau")
	app:resize(400, 500)
	app:frames(30)
	app:set_state("test_tree_offset", 200)
	app:frames(30)
	local all = lines(app)
	local scroll, rail
	for i, l in all do
		if l.line:find("tree \"fixture\"", 1, true) and all[i + 1] then
			scroll = all[i + 1]
		end
		if l.line:find("#file-tree-rail", 1, true) then
			rail = l
		end
	end
	expect(scroll).never.to_be_nil()
	expect(rail).never.to_be_nil()
	assert(scroll and rail)
	-- Beside the faded scroll view, not inside it.
	expect(rail.depth).to_be(scroll.depth)
	-- No bar of the view's own (it would fade with the rows).
	expect(app:has('role:scrollbar')).to_be(false)
	-- Along the view's whole height, at its trailing edge.
	local s, r = box(scroll.line), box(rail.line)
	expect(`{r[2]} {r[4]}`).to_be(`{s[2]} {s[4]}`)
	expect(r[1] + r[3]).to_be(s[1] + s[3])
end)
