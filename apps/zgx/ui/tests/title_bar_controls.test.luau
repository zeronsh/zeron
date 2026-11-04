-- The window's own controls where the platform draws none (Linux,
-- Windows): where macOS's traffic lights stand, in their order, then the
-- ☰ and the bar's controls; none on a tiling compositor unless asked. On
-- ui/titlebar_controls_fixture.luau.

local function open(): any
	local app = mount("titlebar_controls_fixture.luau")
	app:resize(1200, 800)
	app:frames(30)
	return app
end

-- A node's box from the tree: x, y, w, h.
local function box(app: any, target: string): (number, number, number, number)
	local line = app:tree(target, { depth = 0 }):match("[^\n]+") or ""
	local x, y, w, h = line:match(" (%-?[%d%.]+),(%-?[%d%.]+) ([%d%.]+)x([%d%.]+)")
	return tonumber(x) or -1, tonumber(y) or -1, tonumber(w) or -1, tonumber(h) or -1
end

test("close, minimize and maximize stand where macOS's traffic lights do, then the ☰ and the toggle", function()
	local app = open()
	-- macOS's: 14 across, centred 21, 44 and 67 in, 21 down (the bar's middle).
	for i, kind in { "close", "minimize", "maximize" } do
		local x, y, w, h = box(app, `#titlebar-window-{kind}`)
		expect(w).to_be(14)
		expect(h).to_be(14)
		expect(x + w / 2).to_be(21 + (i - 1) * 23)
		expect(y + h / 2).to_be(21)
	end
	expect(app:has('#titlebar >> role:button "Close"')).to_be(true)
	expect(app:has('#titlebar >> role:button "Minimize"')).to_be(true)
	expect(app:has('#titlebar >> role:button "Maximize"')).to_be(true)
	local menu_x = box(app, '#titlebar >> role:button "Menu"')
	expect(menu_x).to_be(88)
	local toggle_x = box(app, '#titlebar >> role:button "Toggle sidebar"')
	expect(toggle_x > menu_x).to_be(true)
end)

test("each control asks the host, and maximize says restore while maximized", function()
	local app = open()
	app:click("#titlebar-window-close")
	app:frames(4)
	expect(app:state("invoked")).to_be("window.close")
	app:click("#titlebar-window-minimize")
	app:frames(4)
	expect(app:state("invoked")).to_be("window.minimize")
	app:click("#titlebar-window-maximize")
	app:frames(4)
	expect(app:state("invoked")).to_be("window.maximize")
	app:command("fixture.maximized")
	app:frames(10)
	expect(app:has('#titlebar >> role:button "Restore"')).to_be(true)
	expect(app:has('#titlebar >> role:button "Maximize"')).to_be(false)
end)

test("on a tiling compositor there are none, and the ☰ starts at the bar's edge", function()
	local app = open()
	app:command("fixture.tiling")
	app:frames(20)
	expect(app:has("#titlebar-window-close")).to_be(false)
	expect(app:has('#titlebar >> role:button "Close"')).to_be(false)
	expect((box(app, '#titlebar >> role:button "Menu"'))).to_be(10)
end)

test("the setting shows them on a tiling compositor, and hides them elsewhere", function()
	local app = open()
	app:command("fixture.tiling")
	app:command("fixture.controls_show")
	app:frames(20)
	expect(app:has("#titlebar-window-close")).to_be(true)
	expect((box(app, '#titlebar >> role:button "Menu"'))).to_be(88)
	app:command("fixture.tiling")
	app:command("fixture.controls_hide")
	app:frames(20)
	expect(app:has("#titlebar-window-close")).to_be(false)
	expect((box(app, '#titlebar >> role:button "Menu"'))).to_be(10)
end)
