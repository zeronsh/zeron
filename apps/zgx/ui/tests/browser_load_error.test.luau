-- The panel's browser tab when a load fails: a page that never came says
-- so over where it would be, with Try again and Dismiss (Escape too), the
-- page under it shown once dismissed, until the next navigation; a page
-- that came and drew (Google's "unusual traffic" check, a 429 WebView2
-- calls a failed navigation) shows as it is, no failure over it. On the
-- panel in ui/panel_fixture.luau, its page stood in for.

local FIELD = 'role:combobox "Address"'
local FAILED = '"Couldn\'t load this page"'
local DISMISS = 'role:button "Dismiss"'
local RETRY = 'role:button "Try again"'

local function eventually(app: any, ok: () -> boolean, ms: number?): boolean
	for _ = 1, math.ceil((ms or 5000) / 50) do
		if ok() then
			return true
		end
		app:wait('"never shown, only waited past"', { timeout_ms = 50 })
	end
	return ok()
end

local function panel(): any
	local app = mount("panel_fixture.luau")
	app:resize(900, 640)
	app:frames(10)
	expect(app:error()).to_be_nil()
	expect(app:wait("#panel-empty", { timeout_ms = 15000 })).to_be(true)
	app:click("#panel-open-browser")
	expect(app:wait(FIELD, { timeout_ms = 15000 })).to_be(true)
	return app
end

local function calls(app: any): string
	return app:state("test_web_calls") or ""
end

-- To google.com/sorry, the page loaded (the field shows its address).
local URL = "https://www.google.com/sorry/index"
local function go(app: any): string
	app:click("#outside")
	app:frames(2)
	app:type(FIELD, URL)
	app:key("Enter")
	app:frames(10)
	expect(eventually(app, function(): boolean
		return app:tree(FIELD):find(`="{string.sub(URL, 1, 36)}`, 1, true) ~= nil
	end)).to_be(true)
	app:frames(5)
	return app:state("test_web_last_id")
end

-- The page moving on, as the host tells it (`started`, `committed`).
local function navigate(app: any, kind: string)
	app:set_state("test_web_nav_kind", kind)
	app:command("test.web_navigate")
	app:frames(5)
end

-- The load failing, as the host has it: the error on the page's state.
local function fail(app: any, id: string, why: string)
	app:set_state(`test_web_error_{id}`, why)
	app:frames(10)
end

-- The page under the overlay is the fixture's stand-in (`page <url>`).
local function page_shown(app: any): boolean
	return app:has("#stub-web")
end

test("browser load error: a page that never came shows the failure, Try again and Dismiss", function()
	local app = panel()
	local id = go(app)
	navigate(app, "started")
	fail(app, id, "A server with the specified hostname could not be found.")
	expect(app:wait(FAILED, { timeout_ms = 3000 })).to_be(true)
	expect(app:has(RETRY)).to_be(true)
	expect(app:has(DISMISS)).to_be(true)
	expect(app:tree('role:checkbox "Annotate"')).to_contain("disabled")
end)

test("browser load error: Dismiss hides the failure and the page stays", function()
	local app = panel()
	local id = go(app)
	navigate(app, "started")
	fail(app, id, "The page could not be loaded.")
	expect(app:wait(FAILED, { timeout_ms = 3000 })).to_be(true)
	app:click(DISMISS)
	app:frames(10)
	expect(app:has(FAILED)).to_be(false)
	expect(app:has(RETRY)).to_be(false)
	expect(page_shown(app)).to_be(true)
	expect(app:has(`"page {URL}"`)).to_be(true)
	-- Only this navigation's: the next one's failure shows again.
	navigate(app, "started")
	expect(app:wait(FAILED, { timeout_ms = 3000 })).to_be(true)
end)

test("browser load error: Escape dismisses", function()
	local app = panel()
	local id = go(app)
	navigate(app, "started")
	fail(app, id, "The page could not be loaded.")
	expect(app:wait(FAILED, { timeout_ms = 3000 })).to_be(true)
	-- (From the address field, nothing typed in it to take back.)
	app:click(FIELD)
	app:frames(2)
	app:key("Escape")
	app:frames(10)
	expect(app:has(FAILED)).to_be(false)
	expect(page_shown(app)).to_be(true)
end)

test("browser load error: Escape from the failure's own buttons dismisses", function()
	local app = panel()
	local id = go(app)
	navigate(app, "started")
	fail(app, id, "The page could not be loaded.")
	expect(app:wait(FAILED, { timeout_ms = 3000 })).to_be(true)
	-- (Tab from the field: past the bar's ⋯ to Try again.)
	app:click(FIELD)
	app:frames(2)
	for _ = 1, 4 do
		if app:tree(RETRY):find("focused", 1, true) then
			break
		end
		app:key("Tab")
		app:frames(2)
	end
	expect(app:tree(RETRY)).to_contain("focused")
	app:key("Escape")
	app:frames(10)
	expect(app:has(FAILED)).to_be(false)
	expect(page_shown(app)).to_be(true)
	expect(calls(app)).never.to_contain("reload")
end)

test("browser load error: Try again reloads the page", function()
	local app = panel()
	local id = go(app)
	navigate(app, "started")
	fail(app, id, "Could not connect to the server.")
	expect(app:wait(FAILED, { timeout_ms = 3000 })).to_be(true)
	app:click(RETRY)
	app:frames(5)
	expect(calls(app)).to_contain("reload")
end)

test("browser load error: a page that came and drew, with an error status, shows no failure", function()
	local app = panel()
	local id = go(app)
	-- WebView2: the server's own page (a 429) committed, then the
	-- navigation reported failed, in the default words.
	navigate(app, "started")
	navigate(app, "committed")
	fail(app, id, "The page could not be loaded.")
	expect(app:has(FAILED)).to_be(false)
	expect(app:has(RETRY)).to_be(false)
	expect(page_shown(app)).to_be(true)
	-- (And the page's controls are the page's: annotate isn't held off.)
	expect(app:tree('role:checkbox "Annotate"')).never.to_contain("disabled")
	-- A reload that never commits is a failure again.
	navigate(app, "started")
	expect(app:wait(FAILED, { timeout_ms = 3000 })).to_be(true)
end)

test("browser load error: no network still says offline, with Dismiss", function()
	local app = panel()
	local id = go(app)
	navigate(app, "started")
	fail(app, id, "The internet connection appears to be offline.")
	expect(app:wait("\"You're offline\"", { timeout_ms = 3000 })).to_be(true)
	expect(app:has(RETRY)).to_be(true)
	expect(app:has(DISMISS)).to_be(true)
end)
