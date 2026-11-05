-- ⇧Tab from the field below a list viewed at its top: the first step lands
-- on the list's last row (r60), and the next go on up from there.
local function focused(app: any): string
	for line in app:tree():gmatch("[^\n]+") do
		if line:find(" focused", 1, true) then
			return line:match('button "([^"]+)"') or "outside the list"
		end
	end
	return "nothing"
end

test("⇧Tab from below a list enters at its last row", function()
	local app = mount("app.luau")
	app:frames(20)
	app:click("#composer")
	app:frames(5)
	local out = {}
	for _ = 1, 4 do
		app:key("shift+Tab")
		app:frames(8)
		table.insert(out, focused(app))
	end
	expect(table.concat(out, " ")).to_be("r60 r58 r56 r54")
end)
