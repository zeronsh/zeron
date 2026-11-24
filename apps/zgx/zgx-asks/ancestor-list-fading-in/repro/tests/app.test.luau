-- The list's rows in view are built once it shows, faded in or not.
local function built(app: any): number
	local n = 0
	for line in app:tree():gmatch("[^\n]+") do
		if line:find('button "r', 1, true) then
			n += 1
		end
	end
	return n
end

test("a list shown by a presence's opacity enter builds the rows in view", function()
	local app = mount("app.luau")
	app:frames(5)
	app:command("show")
	app:frames(60)
	expect(built(app) > 0).to_be(true)
end)

test("the same list shown without the enter builds them (control)", function()
	local app = mount("app.luau")
	app:frames(5)
	app:command("no_fade")
	app:frames(5)
	app:command("show")
	app:frames(60)
	expect(built(app) > 0).to_be(true)
end)
