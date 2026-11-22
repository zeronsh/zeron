-- `@account/plugins`: which plugins run in this window (`plugins.on`, the
-- engine's list as the window loaded it), for the app's own hooks gated on
-- one of Zeron's built-ins. Off by default; a toggle shows and hides the
-- hook in place. Without the plugin loaded (these tests run the app alone)
-- its app-side module is nil even when on: `ui/builtin/<id>.luau` is
-- `false` until the plugin replaces it.

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
	expect(app:has("#gated")).to_be(true)
	app:set_state("plugins_on_seed", { orchestrators = false })
	app:frames(2)
	expect(app:has("#gated")).to_be(false)
	-- Another plugin on is no word on this one.
	app:set_state("plugins_on_seed", { github = true })
	app:frames(2)
	expect(app:has("#gated")).to_be(false)
end)

test("Plugins on: on but not loaded, its module is nil, not the app's stand-in", function()
	local app = mount("plugins_on_fixture.luau")
	app:set_state("plugins_on_seed", { orchestrators = true })
	app:frames(2)
	expect(app:text("#gated")).to_be("no module")
	expect(app:error()).to_be_nil()
end)
