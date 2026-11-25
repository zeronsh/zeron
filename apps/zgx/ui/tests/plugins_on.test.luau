-- `@account/plugins`: which plugins run in this window (`plugins.on`, the
-- engine's list as the window loaded it), for the app's own hooks gated on
-- one of Zeron's built-ins. Off by default; a toggle shows and hides the
-- hook in place. Without the plugin loaded (these tests run the app alone)
-- its app-side module is the app's own `ui/builtin/<id>.luau`: its parts
-- while their modules are still in ui/; once they're the plugin's, `false`,
-- and the built-in reads as off however the window's word says.

test("Plugins on: nothing is on by default", function()
	local app = mount("plugins_on_fixture.luau")
	app:frames(2)
	expect(app:error()).to_be_nil()
	expect(app:has("#gated")).to_be(false)
end)

test("Plugins on: turned on and off, the gated hook follows", function()
	local app = mount("plugins_on_fixture.luau")
	app:set_state("plugins_on_seed", { orchestrators = true })
	app:frames(2)
	-- (On, it shows while its module is the app's own parts; once those
	-- are the plugin's, the plugin isn't loaded here, so it reads as off.)
	expect(app:has("#gated")).to_be(app:text("#standin") == "parts")
	app:set_state("plugins_on_seed", { orchestrators = false })
	app:frames(2)
	expect(app:has("#gated")).to_be(false)
	-- Another plugin on is no word on this one.
	app:set_state("plugins_on_seed", { github = true })
	app:frames(2)
	expect(app:has("#gated")).to_be(false)
end)

test("Plugins on: on but not loaded, its module is the app's own: its parts, or none and off", function()
	local app = mount("plugins_on_fixture.luau")
	app:set_state("plugins_on_seed", { orchestrators = true })
	app:frames(2)
	if app:text("#standin") == "parts" then
		expect(app:text("#gated")).to_be("module")
	else
		expect(app:has("#gated")).to_be(false)
	end
	expect(app:error()).to_be_nil()
end)
