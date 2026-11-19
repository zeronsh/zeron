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
	app:set_state("plugins_view_link_seq", 1)
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

-- A publish notice's "Deploy tokens" (the host's `zeron.open-deploy-tokens`
-- tells `app.link` `{ settings = "plugins", tab = "yours" }`): Settings →
-- Plugins on Yours, where the tokens are, even from a store plugin's page.
test("a notice's Deploy tokens opens Settings → Plugins on Yours, from any page of it", function()
	local app = shell()
	link(app, 1, "focus-timer")
	app:wait(`{PAGE} >> #store-offer`, { timeout_ms = 3000 })
	app:set_state("app.link", { seq = 2, settings = "plugins", tab = "yours" })
	app:wait(`{PAGE} >> "Deploy tokens"`, { timeout_ms = 3000 })
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

-- `zeron://open/plugin/<id>?version=<v>` (src/links.rs tells `version`
-- beside `plugin`): the page with that version chosen. Install acts on it
-- and names it; one the store has but that takes no new installs, or one
-- it hasn't, leaves Install off and says why. A version that doesn't read
-- as one, or keys besides `version`, change nothing.
local function versioned(app: any, seq: number, id: string, version: string?, extra: { [string]: any }?)
	local l: { [string]: any } = { seq = seq, plugin = id, version = version }
	for k, v in extra or {} do
		l[k] = v
	end
	app:set_state("app.link", l)
	app:frames(10)
	app:wait(`{PAGE} >> #store-offer`, { timeout_ms = 3000 })
	-- (Past the link's arming, as the consent window's Allow.)
	app:wait("#never-there", { timeout_ms = 1000 })
	app:frames(10)
end

test("a link that chooses a version opens the page with Install acting on it, named", function()
	local app = shell()
	versioned(app, 1, "focus-timer", "1.0.0")
	local offer = app:tree(`{PAGE} >> #store-offer`)
	expect(offer).to_contain('button "Install 1.0.0"')
	expect(app:tree(`{PAGE} >> #store-install`)).never.to_contain("disabled")
	-- (1.0.0 is verified: no line saying it isn't.)
	expect(app:has(`{PAGE} >> #store-verified-line`)).to_be(false)
	expect(calls(app)).never.to_contain("install")
	-- The head speaks of it: its number in the version line, and where
	-- the person is under the summary, with the way to the newest.
	expect(app:tree(`{PAGE} >> #store-header`)).to_contain("1.0.0")
	expect(app:tree(`{PAGE} >> #store-chosen-line`)).to_contain("This link is for 1.0.0. The newest is 1.2.0.")
	app:click(`{PAGE} >> #store-chosen-line >> role:button "Show the newest"`)
	app:frames(20)
	expect(app:has(`{PAGE} >> #store-chosen-line`)).to_be(false)
	expect(app:tree(`{PAGE} >> #store-offer`)).to_contain('button "Install"')
	expect(app:tree(`{PAGE} >> #store-offer`)).never.to_contain("Install 1.0.0")
	expect(app:tree(`{PAGE} >> #store-header`)).to_contain("1.2.0")
end)

-- The page says the chosen version's state: an unverified one its line,
-- with the verified one to install instead.
test("a link to a version the store hasn't verified says so of that version", function()
	local app = shell()
	app:command("store_stub.bump")
	versioned(app, 1, "focus-timer", "1.3.0")
	expect(app:tree(`{PAGE} >> #store-offer`)).to_contain('button "Install 1.3.0"')
	-- (The newest itself: nothing to say of where the person is.)
	expect(app:has(`{PAGE} >> #store-chosen-line`)).to_be(false)
	expect(app:tree(`{PAGE} >> #store-verified-line`)).to_contain("1.3.0 isn't verified yet")
	expect(app:has(`{PAGE} >> #store-verified-line >> role:button "Install 1.2.0 instead"`)).to_be(true)
end)

test("a link to a version that takes no new installs, or one the store hasn't, leaves Install off and says why", function()
	local app = shell()
	versioned(app, 1, "focus-timer", "1.1.0")
	expect(app:tree(`{PAGE} >> #store-install`)).to_contain("disabled")
	expect(app:tree(`{PAGE} >> #store-offer`)).to_contain('button "Install 1.1.0"')
	-- (Under the summary, where the page's other sentences about a version
	-- are, with the way to the newest; nothing under the button.)
	expect(app:tree(`{PAGE} >> #store-chosen-line`)).to_contain("1.1.0 takes no new installs: replaced by 1.2.0 before it was")
	expect(app:has(`{PAGE} >> #store-chosen-line >> role:button "Show the newest"`)).to_be(true)
	expect(app:has(`{PAGE} >> #store-state`)).to_be(false)
	-- One the store hasn't: asked of it, and said.
	versioned(app, 2, "focus-timer", "9.9.9")
	app:wait(`{PAGE} >> #store-chosen-line >> "The store has no version 9.9.9 of it."`, { timeout_ms = 3000 })
	expect(calls(app)).to_contain("version:focus-timer:9.9.9")
	-- The plugin's ordinary page: the newest, its plain Install, on.
	expect(app:tree(`{PAGE} >> #store-offer`)).to_contain('button "Install"')
	expect(app:tree(`{PAGE} >> #store-offer`)).never.to_contain("Install 9.9.9")
	expect(app:tree(`{PAGE} >> #store-install`)).never.to_contain("disabled")
	expect(app:has(`{PAGE} >> #store-chosen-line >> role:button "Show the newest"`)).to_be(false)
end)

test("a link whose version doesn't read as one opens the page as if it chose none", function()
	local app = shell()
	versioned(app, 1, "focus-timer", "../../etc")
	local offer = app:tree(`{PAGE} >> #store-offer`)
	expect(offer).to_contain('button "Install"')
	expect(offer).never.to_contain("../../etc")
	expect(app:tree(`{PAGE} >> #store-install`)).never.to_contain("disabled")
	expect(calls(app)).never.to_contain("version:")
end)

test("keys besides a link's version change nothing", function()
	local app = shell()
	versioned(app, 1, "focus-timer", "1.0.0", { install = true, from = "web" })
	expect(app:tree(`{PAGE} >> #store-offer`)).to_contain('button "Install 1.0.0"')
	expect(calls(app)).never.to_contain("install")
	-- And a later link to the page without one: the page's own Install.
	versioned(app, 2, "focus-timer", nil, { install = true })
	expect(app:tree(`{PAGE} >> #store-offer`)).to_contain('button "Install"')
	expect(app:tree(`{PAGE} >> #store-offer`)).never.to_contain("Install 1.0.0")
end)

-- Every time a link names a page, its installs wait the consent window's
-- moment again: a second link to a page already open (and armed), with or
-- without a version; and on a page a link opened, every install on it,
-- "Install … instead" and a version row's Install too.
test("a second link to a page already open arms its Install again", function()
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
	local install = `{PAGE} >> #store-install`
	expect(app:tree(install)).never.to_contain("disabled")
	-- A link to the same page comes: it waits again, the press does nothing.
	app:set_state("app.link", { seq = 1, plugin = "focus-timer", version = "1.0.0" })
	app:frames(5)
	expect(app:tree(install)).to_contain("disabled")
	app:click(install)
	app:frames(5)
	expect(calls(app)).never.to_contain("install:")
	app:wait("#never-there", { timeout_ms = 900 })
	app:frames(5)
	expect(app:tree(install)).never.to_contain("disabled")
	-- And again for the next link, with no version.
	app:set_state("app.link", { seq = 2, plugin = "focus-timer" })
	app:frames(5)
	expect(app:tree(install)).to_contain("disabled")
end)

test("on a page a link opened, Install instead and a version row's Install wait too", function()
	local app = shell()
	app:command("store_stub.bump")
	link(app, 1, "focus-timer")
	app:wait(`{PAGE} >> #store-offer`, { timeout_ms = 3000 })
	app:frames(3)
	local instead = `{PAGE} >> #store-verified-line >> role:button "Install 1.2.0 instead"`
	expect(app:tree(instead)).to_contain("disabled")
	local rows = app:tree(`{PAGE} >> role:group "Versions"`)
	local n = 0
	for line in rows:gmatch('[^\n]*button "Install"[^\n]*') do
		n += 1
		expect(line).to_contain("disabled")
	end
	expect(n > 0).to_be(true)
	app:wait("#never-there", { timeout_ms = 900 })
	app:frames(5)
	expect(app:tree(instead)).never.to_contain("disabled")
end)

-- In the app's own frame at a 560 window, Settings' page is narrow: the
-- Plugins tabs and Install don't fit beside each other, so Install takes
-- its own line under the tabs, inside the page, and can be pressed.
test("at a 560 window, Plugins' Install stays inside the page, under its tabs", function()
	local app = shell()
	app:resize(560, 900)
	app:frames(20)
	app:command("shell.settings")
	app:frames(10)
	app:click('#settings-nav >> "Plugins"')
	app:frames(40)
	local tree = app:tree(PAGE)
	local px, _, pw = tree:match("#settings%-page (%-?[%d.]+),(%-?[%d.]+) ([%d.]+)x")
	local _, ty, _, th = tree:match('tablist "Plugins" (%-?[%d.]+),(%-?[%d.]+) ([%d.]+)x([%d.]+)')
	local ix, iy, iw
	local shown = 0
	for line in tree:gmatch('[^\n]*button "Install"[^\n]*') do
		if not line:find("hidden", 1, true) then
			shown += 1
			ix, iy, iw = line:match('button "Install" (%-?[%d.]+),(%-?[%d.]+) ([%d.]+)x')
		end
	end
	assert(px and ty and ix, tree)
	expect(shown).to_be(1)
	expect(tonumber(ix) + tonumber(iw) <= tonumber(px) + tonumber(pw)).to_be(true)
	expect(tonumber(iy) - (tonumber(ty) + tonumber(th))).to_be(16)
end)
