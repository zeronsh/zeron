-- The panel's browser tab: the empty page opens one; an address typed
-- loads (a bare host as https, localhost as http), the tab takes the
-- page's title, and the address rides with the tab across a reload; a
-- link opened twice is one tab; annotation mode runs the picker, a pick is
-- a chip at once (numbered on the page as the chip is), the pick's end
-- raises the note field, whose words stay with the chip, and a chip taken
-- off takes its mark with it. On the panel in ui/panel_fixture.luau, its
-- page stood in for.

local function eventually(app: any, ok: () -> boolean, ms: number?): boolean
	local tries = math.ceil((ms or 15000) / 50)
	for _ = 1, tries do
		if ok() then
			return true
		end
		app:wait('"never shown, only waited past"', { timeout_ms = 50 })
	end
	return ok()
end

local function open_panel(): any
	local app = mount("panel_fixture.luau")
	app:resize(900, 640)
	app:frames(10)
	expect(app:error()).to_be_nil()
	expect(app:wait("#panel-empty", { timeout_ms = 15000 })).to_be(true)
	return app
end

local function tab_titles(app: any): string
	if not app:has("#panel-strip") then
		return ""
	end
	local out: { string } = {}
	for name in app:tree("#panel-strip"):gmatch('tab "([^"]+)"') do
		table.insert(out, name)
	end
	return table.concat(out, ",")
end

local function web_calls(app: any): string
	return app:state("test_web_calls") or ""
end

local function open_browser(app: any)
	app:click("#panel-open-browser")
	expect(app:wait('role:combobox "Address"', { timeout_ms = 15000 })).to_be(true)
end

local function go(app: any, text: string)
	app:type('role:combobox "Address"', text)
	app:key("Enter")
	app:frames(5)
end

-- A pick as the page posts it, with the test's nonce taken from the mode
-- just started: the view's handler checks it, so the test reads it back
-- from the fixture's page handlers through the run script. (The fixture
-- stands the nonce check in: see `test.web_message`.)
local function pick(app: any, nonce: number, element: string, marker: number)
	app:set_state("test_web_message", {
		kind = "zeron.annotation",
		nonce = nonce,
		event = "pick",
		pick = {
			url = "https://example.com/",
			title = "Example Domain",
			element = element,
			selector = `#{marker}`,
			path = { "body", "div" },
			role = "button",
			name = "Save",
			text = "Save",
			html = `<{element}>Save</{element}>`,
			rect = { 10, 20, 120, 32 },
			viewport = { 800, 600 },
			styles = { { "display", "flex" } },
			marker = marker,
		},
	})
	app:command("test.web_message")
	app:frames(3)
end

local function ended(app: any, nonce: number, how: string)
	app:set_state("test_web_message", { kind = "zeron.annotation", nonce = nonce, event = "state", state = how })
	app:command("test.web_message")
	app:frames(3)
end

test("the empty page opens a browser tab; an address loads and names the tab; Escape puts the address back", function()
	local app = open_panel()
	open_browser(app)
	expect(tab_titles(app)).to_be("Browser")
	go(app, "example.com")
	expect(web_calls(app)).to_be("load:https://example.com/")
	expect(eventually(app, function(): boolean
		return tab_titles(app) == "Page at example.com"
	end)).to_be(true)
	-- localhost is http, with its port.
	-- (Focus comes back to the field as the next address is typed: its
	-- text is selected whole, so the typing replaces it.)
	app:click("#outside")
	go(app, "localhost:3000")
	expect(web_calls(app)).to_be("load:https://example.com/,load:http://localhost:3000/")
	-- An address that isn't one says why, loads nothing.
	-- (Focus comes back to the field as the next address is typed: its
	-- text is selected whole, so the typing replaces it.)
	app:click("#outside")
	go(app, "/Users/me/page.html")
	expect(app:has('~"Local files can\'t be opened"')).to_be(true)
	expect(web_calls(app)).to_be("load:https://example.com/,load:http://localhost:3000/")
	-- Escape: back to the page's address.
	app:key("Escape")
	app:frames(2)
	expect(app:has('~"Local files can\'t be opened"')).to_be(false)
end)

test("the address rides with the tab across a reload; a link opened twice is one tab; closing lets the page go", function()
	local app = open_panel()
	open_browser(app)
	go(app, "example.com")
	app:set_state("test_panel_generation", 2)
	app:frames(10)
	expect(app:wait('role:combobox "Address"', { timeout_ms = 15000 })).to_be(true)
	expect(tab_titles(app)).to_be("Page at example.com")
	-- (The page stayed loaded: nothing loaded again.)
	expect(web_calls(app)).to_be("load:https://example.com/")
	-- A chat's link: the tab showing it, else a new one.
	app:set_state("test_open_url", "https://example.com/")
	app:command("test.open_url")
	app:frames(5)
	expect(tab_titles(app)).to_be("Page at example.com")
	app:set_state("test_open_url", "https://zeron.sh/docs")
	app:command("test.open_url")
	app:frames(5)
	expect(eventually(app, function(): boolean
		return tab_titles(app) == "Page at example.com,Page at zeron.sh"
	end)).to_be(true)
	expect(web_calls(app)).to_be("load:https://example.com/,load:https://zeron.sh/docs")
	-- (⌘W closes the panel's tab while the panel has the keyboard.)
	app:click('role:tab "Page at zeron.sh"')
	app:frames(2)
	app:key("cmd+w")
	app:frames(5)
	expect(tab_titles(app)).to_be("Page at example.com")
	expect(string.find(web_calls(app), "close:", 1, true) ~= nil).to_be(true)
end)

test("annotation mode: the picker runs, a pick is a chip and a mark, the end hands the composer the keyboard, a chip gone unmarks", function()
	local app = open_panel()
	open_browser(app)
	go(app, "example.com")
	app:click('role:checkbox "Annotate"')
	app:frames(3)
	expect(string.find(web_calls(app), "run:start", 1, true) ~= nil).to_be(true)
	local nonce = app:state("test_web_nonce")
	expect(nonce).to_be_truthy()
	pick(app, nonce, "button#save", 7)
	expect(string.find(web_calls(app), "run:label(7, 1)", 1, true) ~= nil).to_be(true)
	local cargo = app:state("chat.composer.cargo") or {}
	expect(#cargo).to_be(1)
	expect(cargo[1].kind).to_be("annotation")
	expect(cargo[1].element).to_be("button#save")
	expect(cargo[1].index).to_be(1)
	-- A second, with Shift held: after the first.
	pick(app, nonce, "a.more", 8)
	expect(string.find(web_calls(app), "run:label(8, 2)", 1, true) ~= nil).to_be(true)
	-- The mode ends: no note of its own over the page; the composer takes
	-- the keyboard, to say what to do with them (the shipping app's).
	local focus_before = app:state("chat_focus") or 0
	ended(app, nonce, "done")
	app:frames(5)
	expect(app:has('role:textbox "Note"')).to_be(false)
	expect((app:state("chat_focus") or 0) > focus_before).to_be(true)
	expect(app:tree('role:checkbox "Annotate"')).to_contain("unchecked")
	-- The first chip taken off: the page keeps the second mark alone.
	cargo = app:state("chat.composer.cargo") or {}
	app:set_state("chat.composer.cargo", { cargo[2] })
	app:frames(3)
	expect(string.find(web_calls(app), "run:sync[[2,2]]", 1, true) ~= nil).to_be(true)
end)

test("the keys: ⌘T opens a browser tab beside, ⌘L takes the address, ⌘⇧R reloads; a page's popup is a tab of its own", function()
	local app = open_panel()
	open_browser(app)
	go(app, "example.com")
	-- ⌘T: a second browser tab, on its start page.
	app:command("panel.new_browser_tab")
	app:frames(5)
	expect(tab_titles(app)).to_be("Page at example.com,Browser")
	expect(app:has("#panel-empty")).to_be(false)
	-- ⌘L: the address field has the keyboard.
	app:click("#outside")
	app:command("panel.focus_address")
	app:frames(3)
	expect(string.find(app:tree("role:combobox \"Address\""), "focused", 1, true) ~= nil).to_be(true)
	-- ⌘⇧R on a tab with no page does nothing; on one with a page, reloads.
	app:command("panel.reload_page")
	expect(string.find(web_calls(app), "reload", 1, true)).to_be_nil()
	app:click('role:tab "Page at example.com"')
	app:frames(3)
	app:command("panel.reload_page")
	expect(string.find(web_calls(app), "reload", 1, true) ~= nil).to_be(true)
	-- A popup the page asks for: a new tab on its address, beside this one.
	app:set_state("test_web_window", "https://zeron.sh/docs")
	app:command("test.web_new_window")
	app:frames(5)
	expect(eventually(app, function(): boolean
		return tab_titles(app) == "Page at example.com,Browser,Page at zeron.sh"
	end)).to_be(true)
	-- One that isn't a web address opens nothing.
	app:set_state("test_web_window", "javascript:alert(1)")
	app:command("test.web_new_window")
	app:frames(3)
	expect(tab_titles(app)).to_be("Page at example.com,Browser,Page at zeron.sh")
end)

test("find in page: ⌘F with the tab's keyboard opens the bar, the words light the page and step, Escape closes it", function()
	local app = open_panel()
	open_browser(app)
	go(app, "example.com")
	-- The address field has the keyboard: the panel's, so ⌘F is the tab's.
	app:command("test.find")
	expect(app:state("test_find_in_browser")).to_be("true")
	expect(app:wait('role:textbox "Find in page"', { timeout_ms = 5000 })).to_be(true)
	app:type('role:textbox "Find in page"', "domain")
	expect(eventually(app, function(): boolean
		return string.find(web_calls(app), "find:domain", 1, true) ~= nil
	end, 3000)).to_be(true)
	expect(eventually(app, function(): boolean
		return app:has('"1 of 3"')
	end, 3000)).to_be(true)
	app:key("Enter")
	expect(eventually(app, function(): boolean
		return app:has('"2 of 3"')
	end, 3000)).to_be(true)
	app:key("shift+Enter")
	expect(eventually(app, function(): boolean
		return app:has('"1 of 3"')
	end, 3000)).to_be(true)
	app:key("Escape")
	expect(eventually(app, function(): boolean
		return not app:has('role:textbox "Find in page"')
	end, 3000)).to_be(true)
	expect(string.find(web_calls(app), "find:clear", 1, true) ~= nil).to_be(true)
	-- The keyboard elsewhere (the chat's): ⌘F isn't the tab's.
	app:click("#outside")
	app:frames(2)
	app:command("test.find")
	expect(app:state("test_find_in_browser")).to_be("false")
end)

test("the start page lists no dev servers and says no engine trouble: the panel's home has the session's ports", function()
	local app = open_panel()
	open_browser(app)
	expect(app:has('~"Browse beside the chat"')).to_be(true)
	-- The host's watch answers: two servers, and its proxy in trouble.
	app:set_state("previews", {
		chat = "chat-1",
		ready = true,
		error = "Local previews could not start: Address already in use (os error 48)",
		services = {
			{ id = "svc-web", name = "web", port = 3000, url = "http://web.proj.localhost:27800", project = "proj", device = "mac", cwd = "/proj" },
			{ id = "svc-api", name = "api", port = 8080, url = "http://api.proj.localhost:27800", project = "proj", device = "mac", cwd = "/proj" },
		},
	})
	app:frames(10)
	expect(app:has('~"Browse beside the chat"')).to_be(true)
	expect(app:has('role:button "Open web preview"')).to_be(false)
	expect(app:has('~"Preview your work"')).to_be(false)
	expect(app:has('~"os error"')).to_be(false)
	expect(app:has('~"Address already in use"')).to_be(false)
end)
