-- Settings → Browser and what the browser tab does with it: the page's
-- rows (Open links in moved here from General), history listed, filtered,
-- one let go in place and cleared over a span, a search of your own, the
-- home page refused when it isn't an address, sites' zooms reset; and on
-- the tab, words typed search on the engine chosen, pages reached are kept
-- as history for the account, each site's zoom goes on its page with its
-- percentage in the field (which puts it back), the ⋯ menu's zoom and
-- inspector, and a file the page can't show says so. Settings on the shell
-- in ui/shell_fixture.luau (signed in as Ada: her profile is
-- `zeron-user_ada`); the tab on the panel in ui/panel_fixture.luau, its page
-- stood in for (signed out: `zeron-local`).

local PAGE = "#settings-page"
local NAV = "#settings-nav"
local ADA = "zeron-user_ada"

local function shows(app: any, scope: string, text: string): boolean
	return app:tree(scope):find(text, 1, true) ~= nil
end

-- A node's identity by what it's called: `@id`.
local function node(app: any, scope: string, label: string): string?
	for line in app:tree(scope):gmatch("[^\n]+") do
		if line:find(`"{label}"`, 1, true) then
			return line:match("@(%d+)")
		end
	end
	return nil
end

-- Settings, on one of the Browser heading's pages ("Browsing", "History").
local function settings(page: string?): any
	local app = mount("shell_fixture.luau")
	app:resize(1200, 900)
	app:frames(30)
	app:command("settings_stub.sign_in")
	app:command("shell.settings")
	app:frames(20)
	app:click(`{NAV} >> role:tab "{page or "Browsing"}"`)
	app:frames(30)
	return app
end

local function seed(app: any)
	local now = os.time()
	app:set_state("browser_history", {
		[ADA] = {
			{ url = "https://react.dev/learn", title = "Quick Start – React", at = now - 60, count = 2 },
			{ url = "https://zeron.sh/", title = "Zeron", at = now - 1800, count = 1 },
			{ url = "https://example.com/", title = "Example Domain", at = now - 3 * 86400, count = 1 },
		},
	})
	app:frames(20)
end

test("Browser settings: a heading of its own, Browsing and History, and General no longer has Open links in", function()
	local app = settings()
	expect(app:has(`{NAV} >> "Browser"`)).to_be(true)
	expect(app:has(`{NAV} >> role:tab "History"`)).to_be(true)
	expect(app:state("shell_route_path")).to_be("/settings/browser")
	for _, label in { "Links and search", "Open links in", "Search engine", "Zoom", "Page zoom", "Developer", "Allow inspecting pages" } do
		expect(shows(app, PAGE, label)).to_be(true)
	end
	expect(shows(app, PAGE, "Keep history")).to_be(false)
	-- (No home page: a new tab is the start page.)
	expect(shows(app, PAGE, "New tab opens")).to_be(false)
	app:click(`{NAV} >> role:tab "History"`)
	app:frames(30)
	expect(app:state("shell_route_path")).to_be("/settings/browser_history")
	for _, label in { "Keep history", "Clear history", "No history yet" } do
		expect(shows(app, PAGE, label)).to_be(true)
	end
	-- Nothing kept yet: no filter, the count says so, the list says what goes there.
	expect(shows(app, PAGE, "Nothing kept yet.")).to_be(true)
	expect(shows(app, PAGE, "Pages you visit in the browser show here.")).to_be(true)
	expect(app:has(`{PAGE} >> role:textbox "Search history"`)).to_be(false)
	app:click(`{NAV} >> role:tab "General"`)
	app:frames(20)
	expect(shows(app, PAGE, "Open links in")).to_be(false)
	-- (Where a pull request opens is the GitHub plugin's setting.)
	expect(shows(app, PAGE, "Open pull requests in")).to_be(false)
end)

test("Browser settings: history lists, filters, and lets one page go in place", function()
	local app = settings("History")
	seed(app)
	expect(app:wait(`{PAGE} >> "Quick Start – React"`, { timeout_ms = 5000 })).to_be(true)
	expect(shows(app, PAGE, "Zeron")).to_be(true)
	expect(shows(app, PAGE, "Example Domain")).to_be(true)
	expect(shows(app, PAGE, "3 pages kept.")).to_be(true)
	-- Laid out by day, the latest first, each with the time it was visited.
	expect(shows(app, PAGE, "Today")).to_be(true)
	expect(shows(app, PAGE, " AM\"") or shows(app, PAGE, " PM\"")).to_be(true)
	-- The filter: by address or title.
	app:type(`{PAGE} >> role:textbox "Search history"`, "react")
	app:frames(20)
	expect(shows(app, PAGE, "Quick Start – React")).to_be(true)
	expect(shows(app, PAGE, "Example Domain")).to_be(false)
	app:type(`{PAGE} >> role:textbox "Search history"`, "nothing-like-it")
	app:frames(20)
	expect(shows(app, PAGE, "No pages match.")).to_be(true)
	app:set_state("settings_browser_history_query", "")
	app:frames(20)
	-- One let go: the others stay the same rows.
	local zeron = node(app, PAGE, "Zeron")
	expect(zeron).never.to_be_nil()
	-- (Its × shows as the row is pointed at, in its time's place.)
	local remove = `{PAGE} >> role:button "Remove Example Domain from history"`
	expect(app:tree(remove)).to_contain("hidden")
	app:hover(`{PAGE} >> "Example Domain"`)
	app:frames(5)
	expect(app:tree(remove)).never.to_contain("hidden")
	app:click(remove)
	app:frames(20)
	expect(shows(app, PAGE, "Example Domain")).to_be(false)
	expect(node(app, PAGE, "Zeron")).to_be(zeron)
	local kept = (app:state("browser_history") :: any)[ADA]
	expect(#kept).to_be(2)
end)

test("Browser settings: Clear history clears the span chosen, then all of it", function()
	local app = settings("History")
	seed(app)
	-- The last hour: the two recent go, the one three days back stays.
	app:set_state("settings_browser_span", "hour")
	app:frames(10)
	app:click("#settings-browser-clear")
	app:frames(20)
	local kept = (app:state("browser_history") :: any)[ADA]
	expect(#kept).to_be(1)
	expect(kept[1].url).to_be("https://example.com/")
	expect(shows(app, PAGE, "1 page kept.")).to_be(true)
	app:set_state("settings_browser_span", "all")
	app:frames(10)
	app:click("#settings-browser-clear")
	app:frames(20)
	expect((app:state("browser_history") :: any)[ADA]).to_be_nil()
	expect(shows(app, PAGE, "Nothing kept yet.")).to_be(true)
end)

test("Browser settings: a search of your own, checked for where its words go", function()
	local app = settings()
	expect(app:has("#settings-browser-custom-engine")).to_be(false)
	app:set_state("browser_search_engine", "custom")
	app:frames(20)
	expect(app:has("#settings-browser-custom-engine")).to_be(true)
	app:type("#settings-browser-custom-engine", "https://search.example/%s")
	app:key("Enter")
	app:frames(10)
	-- (%s in the path is fine; one in the host is not.)
	expect(app:state("browser_custom_search")).to_be("https://search.example/%s")
	app:set_state("browser_custom_search", "https://%s.example/")
	app:frames(10)
	-- (The tree cuts a long line short: its start.)
	expect(shows(app, PAGE, "Use a web address with %s after the site")).to_be(true)
end)

test("Browser settings: sites with their own zoom, reset", function()
	local app = settings()
	expect(app:has("#settings-browser-reset-zoom")).to_be(false)
	app:set_state("browser_site_zoom", { ["example.com"] = 1.25, ["zeron.sh"] = 0.9 })
	app:frames(20)
	expect(shows(app, PAGE, "2 sites with their own zoom")).to_be(true)
	expect(shows(app, PAGE, "125%")).to_be(true)
	-- One back to the default, in place; then all of them.
	app:click(`{PAGE} >> role:button "Reset zoom for zeron.sh"`)
	app:frames(20)
	expect(shows(app, PAGE, "1 site with its own zoom")).to_be(true)
	expect((app:state("browser_site_zoom") :: any)["zeron.sh"]).to_be_nil()
	app:click("#settings-browser-reset-zoom")
	app:frames(20)
	expect(app:has("#settings-browser-reset-zoom")).to_be(false)
end)

-- The tab ------------------------------------------------------------------

local FIELD_ROLE = 'role:combobox "Address"'

local function panel(): any
	local app = mount("panel_fixture.luau")
	app:resize(900, 640)
	app:frames(10)
	expect(app:error()).to_be_nil()
	expect(app:wait("#panel-empty", { timeout_ms = 15000 })).to_be(true)
	app:click("#panel-open-browser")
	expect(app:wait('role:combobox "Address"', { timeout_ms = 15000 })).to_be(true)
	return app
end

local function eventually(app: any, ok: () -> boolean, ms: number?): boolean
	for _ = 1, math.ceil((ms or 5000) / 50) do
		if ok() then
			return true
		end
		app:wait('"never shown, only waited past"', { timeout_ms = 50 })
	end
	return ok()
end

-- An address typed and taken; once the page answers, the field shows its
-- address (so the next typed replaces it), and the page's title has come.
local function go(app: any, text: string, lands: string?)
	-- (What the field holds, replaced: it selects all as it takes the keyboard.)
	app:click("#outside")
	app:frames(2)
	app:type('role:combobox "Address"', text)
	app:key("Enter")
	app:frames(10)
	if lands then
		expect(eventually(app, function(): boolean
			-- (The tree cuts a long value short: its start.)
			return app:tree('role:combobox "Address"'):find(`="{string.sub(lands, 1, 36)}`, 1, true) ~= nil
		end)).to_be(true)
		app:frames(5)
	end
end

local function history(app: any): { any }?
	local all = app:state("browser_history") :: any
	return if all then all["zeron-local"] else nil
end

local function calls(app: any): string
	return app:state("test_web_calls") or ""
end

test("browser tab: words search on the engine chosen; an address loads", function()
	local app = panel()
	go(app, "react hooks", "https://www.google.com/search?q=react+hooks")
	app:set_state("browser_search_engine", "duckduckgo")
	go(app, "zeron", "https://duckduckgo.com/?q=zeron")
	go(app, "example.com", "https://example.com/")
	go(app, "localhost:3000", "http://localhost:3000/")
	-- (A path stays refused: there's no search a file should become.)
	go(app, "/etc/hosts")
	expect(calls(app)).never.to_contain("hosts")
	expect(calls(app)).never.to_contain("zoom")
end)

test("browser tab: pages reached are kept as the account's history, with their titles", function()
	local app = panel()
	go(app, "example.com", "https://example.com/")
	expect(eventually(app, function(): boolean
		local h = history(app)
		return h ~= nil and h[1].title == "Page at example.com"
	end)).to_be(true)
	expect((history(app) :: any)[1].url).to_be("https://example.com/")
	go(app, "zeron.sh", "https://zeron.sh/")
	expect(eventually(app, function(): boolean
		local h = history(app)
		return h ~= nil and h[1].url == "https://zeron.sh/"
	end)).to_be(true)
	expect((history(app) :: any)[2].url).to_be("https://example.com/")
	-- Off, nothing more is kept.
	app:set_state("browser_keep_history", false)
	go(app, "react.dev", "https://react.dev/")
	app:wait('"never shown, only waited past"', { timeout_ms = 300 })
	expect((history(app) :: any)[1].url).to_be("https://zeron.sh/")
end)

test("browser tab: a site's zoom goes on its page, its percentage in the field puts it back", function()
	local app = panel()
	app:set_state("browser_site_zoom", { ["example.com"] = 1.25 })
	go(app, "example.com", "https://example.com/")
	expect(eventually(app, function(): boolean
		return string.find(calls(app), "zoom:125", 1, true) ~= nil
	end)).to_be(true)
	local id = app:state("test_web_last_id")
	local pill = `#panel-browser-zoom-{string.gsub(id, "^panel%-browser%-", "")}`
	expect(app:has(pill)).to_be(true)
	expect(app:tree(pill)).to_contain("125%")
	app:click(pill)
	app:frames(10)
	expect(calls(app)).to_contain("zoom:100")
	expect(app:has(pill)).to_be(false)
	expect((app:state("browser_site_zoom") :: any)["example.com"]).to_be_nil()
	-- ⌘+ is the page's while it has the keyboard, and kept for the site.
	app:set_state(`test_web_focused_{id}`, true)
	app:command("panel.zoom_in")
	app:frames(10)
	expect((app:state("browser_site_zoom") :: any)["example.com"]).to_be(1.1)
	app:command("panel.zoom_reset")
	app:frames(10)
	expect((app:state("browser_site_zoom") :: any)["example.com"]).to_be_nil()
	-- Not while the keyboard is elsewhere.
	app:set_state(`test_web_focused_{id}`, false)
	app:command("panel.zoom_in")
	app:frames(10)
	expect((app:state("browser_site_zoom") :: any)["example.com"]).to_be_nil()
end)

test("browser tab: the ⋯ menu opens the inspector and zooms", function()
	local app = panel()
	go(app, "example.com", "https://example.com/")
	app:click('role:button "More"')
	expect(app:wait('role:menuitem "Inspect Page"', { timeout_ms = 3000 })).to_be(true)
	-- (A menu takes no pick in its first moments: time passes.)
	app:wait('"nothing"', { timeout_ms = 400 })
	app:click('role:menuitem "Inspect Page"')
	-- (It blinks, the menu closes, then it runs: real time.)
	expect(eventually(app, function(): boolean
		return string.find(calls(app), "devtools", 1, true) ~= nil
	end, 3000)).to_be(true)
	app:wait('"nothing"', { timeout_ms = 300 })
	-- Inspecting off: no such item. Zoom keeps the menu open.
	app:set_state("browser_inspectable", false)
	app:click('role:button "More"')
	expect(app:wait('role:menuitem "Zoom In"', { timeout_ms = 3000 })).to_be(true)
	expect(app:has('role:menuitem "Inspect Page"')).to_be(false)
	app:wait('"nothing"', { timeout_ms = 400 })
	app:click('role:menuitem "Zoom In"')
	app:frames(10)
	expect((app:state("browser_site_zoom") :: any)["example.com"]).to_be(1.1)
	expect(app:has('role:menuitem "Zoom In"')).to_be(true)
	-- Open in the default browser is the menu's (not a button on the bar).
	app:click('role:menuitem "Open in Browser"')
	expect(eventually(app, function(): boolean
		return string.find(calls(app), "external:https://example.com/", 1, true) ~= nil
	end, 3000)).to_be(true)
end)

test("browser tab: the bar is back, forward, reload, the address, then the page's controls", function()
	local app = panel()
	go(app, "example.com", "https://example.com/")
	local function x(target: string): number
		return tonumber(app:tree(target):match(" (%d+),%d+ %d+x%d+")) or -1
	end
	local fwd, reload, field = x('role:button "Forward"'), x('role:button "Reload"'), x(FIELD_ROLE)
	expect(fwd < reload and reload < field).to_be(true)
	expect(app:has('#panel-browser-bar-tab1 >> role:button "Open in Browser"')).to_be(false)
end)

test("browser tab: a file the page can't show says so, and opens in the default browser", function()
	local app = panel()
	go(app, "example.com/archive.zip", "https://example.com/archive.zip")
	local id = app:state("test_web_last_id")
	app:set_state(`test_web_error_{id}`, "Frame load interrupted")
	app:frames(20)
	expect(app:wait("\"This file can’t be shown here\"", { timeout_ms = 3000 })).to_be(true)
	app:click('role:button "Open in Browser"')
	app:frames(10)
	expect(calls(app)).to_contain("external:https://example.com/archive.zip")
end)

-- Suggestions ----------------------------------------------------------------

local FIELD = 'role:combobox "Address"'

local function seeded(): any
	local app = panel()
	local now = os.time()
	app:set_state("browser_history", {
		["zeron-local"] = {
			{ url = "https://react.dev/reference/react/useEffect", title = "useEffect – React", at = now - 60, count = 3 },
			{ url = "https://react.dev/", title = "React", at = now - 3600, count = 6 },
			{ url = "https://github.com/zeronsh/zeron/pull/1187", title = "Browser settings · Pull Request #1187", at = now - 600, count = 2 },
			{ url = "https://developer.apple.com/documentation/webkit", title = "WebKit | Apple Developer", at = now - 86400 * 3, count = 1 },
		},
	})
	app:frames(5)
	return app
end

-- The rows the list shows, in order, by what each is called.
local function rows(app: any): { string }
	local out = {}
	for name in app:tree():gmatch('option "([^"]+)"') do
		table.insert(out, name)
	end
	return out
end

local function field(app: any): string
	return app:text(FIELD) or ""
end

-- Typed into the field (what it held replaced: it selects all as it takes
-- the keyboard). The test host inserts text, not key by key, so Backspace's
-- own editing is checked in a real window (zgx inspect), not here.
local function keys(app: any, text: string)
	app:click("#outside")
	app:type(FIELD, text)
	app:frames(8)
end

test("suggestions: the field finishes the site you visit, the list under it ranked", function()
	local app = seeded()
	-- Taking the keyboard opens nothing.
	app:click("#outside")
	app:click(FIELD)
	app:frames(10)
	expect(#rows(app)).to_be(0)
	keys(app, "rea")
	-- The site finished in the field, the rest selected (typing on replaces it).
	expect(field(app)).to_be("react.dev")
	local list = rows(app)
	expect(list[1]).to_be("react.dev")
	-- Then the pages, the ones whose address starts with it first.
	expect(list[2]).to_be("useEffect – React")
	expect(table.find(list, "WebKit | Apple Developer")).to_be_nil()
	-- Enter: the site.
	app:key("Enter")
	app:frames(10)
	expect(calls(app)).to_contain("load:https://react.dev/")
	expect(#rows(app)).to_be(0)
end)

test("suggestions: ↓ fills the field with the row lit; Escape puts back what was typed and closes", function()
	local app = seeded()
	keys(app, "git")
	expect(field(app)).to_be("github.com")
	expect(rows(app)[2]).to_be("Browser settings · Pull Request #1187")
	app:key("ArrowDown")
	app:frames(5)
	expect(field(app)).to_be("github.com/zeronsh/zeron/pull/1187")
	-- ↑ back to the first: the site finished again.
	app:key("ArrowUp")
	app:frames(5)
	expect(field(app)).to_be("github.com")
	app:key("Escape")
	app:frames(5)
	expect(field(app)).to_be("git")
	expect(#rows(app)).to_be(0)
	-- Escape again: the field's own, the page's address back.
	app:key("Escape")
	app:frames(5)
	expect(field(app)).to_be("")
end)

test("suggestions: words search first, a page below them goes with ↓ and Enter", function()
	local app = seeded()
	keys(app, "apple webkit")
	local list = rows(app)
	expect(list[1]).to_be("apple webkit")
	expect(list[2]).to_be("WebKit | Apple Developer")
	expect(app:tree()).to_contain("Search Google")
	app:key("ArrowDown")
	app:frames(5)
	app:key("Enter")
	app:frames(10)
	expect(calls(app)).to_contain("load:https://developer.apple.com/documentation/webkit")
end)

test("suggestions: Shift-Backspace lets the lit page out of history", function()
	local app = seeded()
	keys(app, "zeronsh")
	expect(rows(app)[2]).to_be("Browser settings · Pull Request #1187")
	app:key("ArrowDown")
	app:frames(5)
	app:key("shift+Backspace")
	app:frames(10)
	expect(table.find(rows(app), "Browser settings · Pull Request #1187")).to_be_nil()
	local kept = (app:state("browser_history") :: any)["zeron-local"]
	for _, v in kept do
		expect(v.url).never.to_be("https://github.com/zeronsh/zeron/pull/1187")
	end
end)

test("suggestions: letters inside a word don't match; a word's start does", function()
	local app = seeded()
	keys(app, "elop")
	expect(table.find(rows(app), "WebKit | Apple Developer")).to_be_nil()
	keys(app, "dev")
	expect(table.find(rows(app), "WebKit | Apple Developer")).never.to_be_nil()
end)
