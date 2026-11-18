-- `zeron://open/plugin/<id>` (src/links.rs tells the scripts `app.link`'s
-- `plugin`): the shell opens Settings → Plugins on that store plugin's
-- page with Install ready, whether Zeron was elsewhere or already there,
-- and never installs anything; an id the store doesn't have says so. On
-- the shell in ui/shell_fixture.luau (its Plugins loaded), the store
-- answered by ui/store_stub.luau.

local PAGE = "#settings-page"

local function calls(app: any): string
	local ok, list = pcall(function()
		return app:state("store_stub_calls")
	end)
	return if ok and list then table.concat(list, "\n") else ""
end

local function shell(): any
	local app = mount("shell_fixture.luau")
	app:resize(1200, 900)
	app:frames(30)
	app:command("settings_stub.sign_in")
	-- (Plugins loaded beside the shell, as the app does.)
	app:set_state("shell_fixture.plugins", true)
	app:frames(10)
	return app
end

local function link(app: any, seq: number, id: string)
	app:set_state("app.link", { seq = seq, plugin = id })
	app:frames(10)
end

test("a plugin's link opens its page in Settings → Plugins, Install ready, installing nothing", function()
	local app = shell()
	link(app, 1, "focus-timer")
	app:wait(`{PAGE} >> #store-offer`, { timeout_ms = 3000 })
	app:frames(20)
	expect(app:state("shell_route_path")).to_contain("settings")
	expect(app:tree(PAGE)).to_contain('heading "Focus timer"')
	expect(app:has(`{PAGE} >> #store-offer >> role:button "Install"`)).to_be(true)
	expect(calls(app)).to_contain("detail:focus-timer")
	expect(calls(app)).never.to_contain("install")
	-- (The shell fixture's host has no `plugins.check_updates`, which the
	-- Plugins page asks as it opens on a device, link or not.)
end)

test("a link while Settings → Plugins is open shows that plugin, and Back is Browse", function()
	local app = shell()
	app:command("shell.settings")
	app:frames(10)
	app:click('#settings-nav >> "Plugins"')
	app:frames(30)
	link(app, 1, "focus-timer")
	app:wait(`{PAGE} >> #store-offer`, { timeout_ms = 3000 })
	link(app, 2, "cost-meter")
	app:wait(`{PAGE} >> "Cost meter"`, { timeout_ms = 3000 })
	app:frames(20)
	expect(app:tree(PAGE)).to_contain('heading "Cost meter"')
	app:click(`{PAGE} >> role:button "Back to Plugins"`)
	app:wait(`{PAGE} >> #store-card`, { timeout_ms = 3000 })
end)

test("a link to a plugin the store doesn't have opens its page's not-found", function()
	local app = shell()
	link(app, 1, "no-such-plugin")
	app:wait(`{PAGE} >> "This plugin isn't in the store"`, { timeout_ms = 3000 })
end)

test("each link is followed once, and a reload follows none again", function()
	local app = shell()
	link(app, 1, "focus-timer")
	app:wait(`{PAGE} >> #store-offer`, { timeout_ms = 3000 })
	app:click(`{PAGE} >> role:button "Back to Plugins"`)
	app:wait(`{PAGE} >> #store-card`, { timeout_ms = 3000 })
	-- The same link again (its seq unchanged) is not followed again.
	link(app, 1, "focus-timer")
	app:frames(30)
	expect(app:has(`{PAGE} >> #store-offer`)).to_be(false)
end)

test("a later visit to Settings opens on the tabs, not on the plugin a link showed", function()
	local app = shell()
	link(app, 1, "focus-timer")
	app:wait(`{PAGE} >> #store-offer`, { timeout_ms = 3000 })
	app:key("Escape")
	app:frames(30)
	app:command("shell.settings")
	app:frames(10)
	app:click('#settings-nav >> "Plugins"')
	app:frames(30)
	expect(app:has(`{PAGE} >> #store-offer`)).to_be(false)
end)

test("opened by a link, the page's action takes no click for a moment, as the consent window's Allow; then it does", function()
	local app = shell()
	link(app, 1, "focus-timer")
	app:wait(`{PAGE} >> #store-offer`, { timeout_ms = 3000 })
	app:frames(3)
	local install = `{PAGE} >> #store-slot >> role:button "Install"`
	-- (Come forward under a click: not armed yet, the press does nothing.)
	expect(app:tree(install)).to_contain("disabled")
	app:click(install)
	app:frames(5)
	expect(calls(app)).never.to_contain("install:")
	-- After the consent window's own wait (its ARM_MS, 800), it takes one.
	app:wait("#never-there", { timeout_ms = 900 })
	app:frames(5)
	expect(app:tree(install)).never.to_contain("disabled")
end)

test("a link's page: a press inside the wait installs nothing, one after it installs", function()
	-- (The plugins page's own fixture, whose install is noted: the page as
	-- the link's route leaves it, nav's own record of the link.)
	local app = mount("plugins_store_fixture.luau")
	app:resize(1000, 900)
	app:frames(10)
	app:set_state("plugins_view_linked", "focus-timer")
	app:set_state("plugins_view", "store:focus-timer")
	app:wait("#store-offer", { timeout_ms = 3000 })
	app:frames(3)
	local install = '#store-slot >> role:button "Install"'
	app:click(install)
	app:frames(5)
	expect(calls(app)).never.to_contain("install:")
	app:wait("#never-there", { timeout_ms = 900 })
	app:frames(5)
	app:click(install)
	app:frames(5)
	expect(calls(app)).to_contain("install:focus-timer")
end)

test("opened from Browse, the page's action takes a click at once", function()
	local app = shell()
	app:command("shell.settings")
	app:frames(10)
	app:click('#settings-nav >> "Plugins"')
	app:frames(30)
	app:click(`{PAGE} >> role:tab "Browse"`)
	app:wait(`{PAGE} >> #store-rows`, { timeout_ms = 3000 })
	app:frames(10)
	app:click(`{PAGE} >> #store-card >> role:button "Focus timer"`)
	app:wait(`{PAGE} >> #store-offer`, { timeout_ms = 3000 })
	app:frames(3)
	expect(app:tree(`{PAGE} >> #store-slot >> role:button "Install"`)).never.to_contain("disabled")
end)

-- A publish notice's "Deploy keys" (the host's `zeron.open-deploy-keys`
-- tells `app.link` `{ settings = "plugins", tab = "yours" }`): Settings →
-- Plugins on Yours, where the keys are, even from a store plugin's page.
test("a notice's Deploy keys opens Settings → Plugins on Yours, from any page of it", function()
	local app = shell()
	link(app, 1, "focus-timer")
	app:wait(`{PAGE} >> #store-offer`, { timeout_ms = 3000 })
	app:set_state("app.link", { seq = 2, settings = "plugins", tab = "yours" })
	app:wait(`{PAGE} >> "Deploy keys"`, { timeout_ms = 3000 })
	app:frames(20)
	expect(app:has(`{PAGE} >> #store-offer`)).to_be(false)
	expect(app:has(`{PAGE} >> #keys-new`)).to_be(true)
	expect(app:state("plugins_tab")).to_be("yours")
	-- A tab the page hasn't is no tab at all: Settings opens, as it was.
	app:set_state("app.link", { seq = 3, settings = "plugins", tab = "nowhere" })
	app:frames(20)
	expect(app:state("plugins_tab")).to_be("yours")
	expect(app:has(`{PAGE} >> #keys-new`)).to_be(true)
end)
