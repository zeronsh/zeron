-- How far the second word starts after the first ends.
local function apart(app: any, id: string): number
	local tree = app:tree(`#{id}`)
	local x1, w1 = tree:match(`{id}%-one" ([%d.]+),[%d.]+ ([%d.]+)x`)
	local x2 = tree:match(`{id}%-two" ([%d.]+),`)
	return (tonumber(x2) or 0) - ((tonumber(x1) or 0) + (tonumber(w1) or 0))
end

test("gap_x given a token spaces as the token's number does", function()
	local app = mount("app.luau")
	app:frames(3)
	expect(apart(app, "token")).to_be(apart(app, "number"))
end)

test("gap_x given a number, and gap given the token, space by it (controls)", function()
	local app = mount("app.luau")
	app:frames(3)
	expect(apart(app, "number")).to_be(4)
	expect(apart(app, "gap")).to_be(4)
end)

local function h_of(app: any, id: string): number
	return tonumber((app:tree(`#{id}`):match(" [%d.]+,[%d.]+ [%d.]+x([%d.]+)"))) or 0
end

test("min_h given a token holds the row to it, as the number does", function()
	local app = mount("app.luau")
	app:frames(3)
	expect(h_of(app, "min-token")).to_be(h_of(app, "min-number"))
end)

test("min_h given a number, and h given the token, size by it (controls)", function()
	local app = mount("app.luau")
	app:frames(3)
	expect(h_of(app, "min-number")).to_be(32)
	expect(h_of(app, "h-token")).to_be(32)
end)
