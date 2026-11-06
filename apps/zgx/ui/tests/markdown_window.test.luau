-- A windowed reply's heights (ds/markdown's `window`).

local function height(app: any, id: string): number
	local h = app:tree(id):match("^%s*@%d+[^\n]- [%-%d.]+,[%-%d.]+ [%d.]+x([%d.]+)")
	return tonumber(h) :: number
end

test("a run judged away before it hears its own size keeps the height it was laid out at, not a guess", function()
	local app = mount("markdown_window_fixture.luau")
	app:frames(30)
	expect(app:error()).to_be_nil()
	local whole, row = height(app, "#whole"), height(app, "#row")
	expect(row).to_be(whole)
end)
