-- Zeron's light theme on its solid window: muted text darkens to stay legible
-- there, glass keeps its own tones, and the dark theme is untouched. Driven
-- through Settings on the shell in ui/shell_fixture.luau.

local PAGE = "#settings-page"
local NAV = "#settings-nav"

local function appearance(scheme: string): any
	local app = mount("shell_fixture.luau")
	app:resize(1200, 900)
	app:frames(30)
	app:command(`fixture.{scheme}`)
	app:frames(5)
	app:command("settings_stub.sign_in")
	app:command("shell.settings")
	app:frames(20)
	app:click(`{NAV} >> role:tab "Appearance"`)
	app:frames(30)
	return app
end

local function muted(app: any): (string?, string?)
	app:command("settings_stub.read_theme")
	app:frames(2)
	local t = app:state("settings_stub_window_theme") or {}
	return t.secondary, t.tertiary
end

test("light: muted text is the solid window's when opaque, and glass's when frosted", function()
	local app = appearance("light")
	expect(muted(app)).to_be(nil)
	app:click(`{PAGE} >> role:switch "Frosted glass"`)
	app:frames(10)
	local secondary, tertiary = muted(app)
	expect(secondary).to_be("$zeron.solid.secondary")
	expect(tertiary).to_be("$zeron.solid.tertiary")
	app:click(`{PAGE} >> role:switch "Frosted glass"`)
	app:frames(10)
	expect(muted(app)).to_be(nil)
end)

test("dark: muted text keeps the theme's own when opaque", function()
	local app = appearance("dark")
	app:click(`{PAGE} >> role:switch "Frosted glass"`)
	app:frames(10)
	expect(muted(app)).to_be(nil)
end)
