-- The link's box: its first fragment, as the tree reports it.
local function link_w(app: any, id: string): number
	local line = app:tree(`#{id} >> role:link`):match("[^\n]+") or ""
	return tonumber(line:match(" [%d.]+,[%d.]+ ([%d.]+)x")) or 0
end

local function whole(app: any): number
	-- ("Publish…" on one line, as the control draws it.)
	return link_w(app, "inside")
end

test("a link span that doesn't fit after the words moves whole to the next line", function()
	local app = mount("app.luau")
	app:frames(5)
	expect(link_w(app, "at-boundary")).to_be(whole(app))
end)

test("the same link, the break falling inside the words' span, stays whole (control)", function()
	local app = mount("app.luau")
	app:frames(5)
	expect(whole(app) > 50).to_be(true)
end)
