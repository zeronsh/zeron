local function y_of(app: any, sel: string): number
	return tonumber((app:tree(sel):match(" [%d.]+,([%-%d.]+) "))) or 0
end

test("a section asked into view shows its head", function()
	local app = mount("app.luau")
	app:frames(3)
	app:scroll("#view", 1e6)
	app:frames(3)
	app:command("reveal")
	app:frames(5)
	local head = y_of(app, "#head")
	expect(head >= 0 and head < 300).to_be(true)
end)

test("focus brings the focused button into view (control)", function()
	local app = mount("app.luau")
	app:frames(3)
	app:scroll("#view", 1e6)
	app:frames(3)
	app:command("reveal")
	app:frames(5)
	local foot = y_of(app, "#foot")
	expect(foot >= 0 and foot < 300).to_be(true)
end)
