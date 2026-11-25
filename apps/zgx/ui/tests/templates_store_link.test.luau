-- `zeron://open/template/<id>` (src/links.rs tells the scripts `app.link`'s
-- `template`): the shell opens Settings → Templates on that template's page,
-- never installing anything, armed a moment after it comes forward as the
-- plugins' is. On the shell in ui/shell_fixture.luau, the store answered by
-- ui/template_store_stub.luau.

local PAGE = "#settings-page"

local function shell(): any
	local app = mount("shell_fixture.luau")
	app:resize(1200, 900)
	app:frames(30)
	app:command("settings_stub.sign_in")
	app:set_state("shell_fixture.plugins", true)
	app:frames(10)
	return app
end

local function calls(app: any): string
	return table.concat(app:state("template_stub_calls") or {}, "\n")
end

test("a template's link opens its page in Settings → Templates, not armed at once, installing nothing", function()
	local app = shell()
	app:set_state("app.link", { seq = 1, template = "midnight" })
	app:wait(`{PAGE} >> #store-offer`, { timeout_ms = 3000 })
	app:frames(5)
	expect(app:state("shell_route_path")).to_contain("templates")
	expect(app:tree(PAGE)).to_contain('heading "Midnight"')
	expect(app:tree(PAGE)).to_contain("Runs as Zeron itself")
	expect(app:tree(`{PAGE} >> #store-install`)).to_contain("disabled")
	expect(calls(app)).to_contain("detail:midnight")
	app:wait("#never-there", { timeout_ms = 900 })
	app:frames(5)
	expect(app:tree(`{PAGE} >> #store-install`)).never.to_contain("disabled")
	-- Never a plugin's page, whatever the id.
	expect(app:state("plugins_view")).to_be("catalog")
end)

test("a link while Templates is open shows that template, the version it chose, and Back is Browse", function()
	local app = shell()
	app:command("shell.settings")
	app:frames(10)
	app:click('#settings-nav >> "Templates"')
	app:frames(30)
	app:set_state("app.link", { seq = 1, template = "ledger", version = "1.1.0" })
	app:wait(`{PAGE} >> #store-offer`, { timeout_ms = 3000 })
	app:wait("#never-there", { timeout_ms = 400 })
	app:frames(10)
	expect(app:tree(PAGE)).to_contain('heading "Ledger"')
	expect(app:tree(`{PAGE} >> #store-install`)).to_contain("1.1.0")
	app:click(`{PAGE} >> role:button "Back to Templates"`)
	app:wait(`{PAGE} >> #store-cards`, { timeout_ms = 3000 })
end)

test("a plugin's link and a template's each open their own, and an id the store hasn't says so", function()
	local app = shell()
	app:set_state("app.link", { seq = 1, template = "no-such-template" })
	app:wait(`{PAGE} >> "This template isn't in the store"`, { timeout_ms = 3000 })
	app:set_state("app.link", { seq = 2, plugin = "focus-timer" })
	app:wait(`{PAGE} >> #store-offer`, { timeout_ms = 3000 })
	app:frames(20)
	expect(app:tree(PAGE)).to_contain('heading "Focus timer"')
	expect(app:tree(PAGE)).never.to_contain("Runs as Zeron itself")
end)
