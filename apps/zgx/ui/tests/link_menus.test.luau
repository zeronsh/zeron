-- Every link's context menu, by its kind (ds/links, screens/chat/link_menu),
-- shown by the window's one host (ds/menu_at); and a web link keeping its
-- href (its cursor, role and menu) while the app takes its click. On
-- ui/chat_fixture.luau's link turn: a prompt mentioning a file, a chat and
-- the web; a reply linking files (one outside the chat's /work/atlas) and
-- the web.

local function setup(): any
	local app = mount("chat_fixture.luau")
	app:resize(900, 700)
	app:command("go_s2")
	app:frames(30)
	app:command("set_links")
	app:command("link_turn")
	app:frames(30)
	return app
end

-- (`has` sees a closed menu's rows too: what shows is read from the tree,
-- whose closed rows say `hidden`. The menu opens, and a choice acts, a
-- moment later: time passed for it, a frame's wait at a time.)
local function item(name: string): string
	return `menuitem "{name}"`
end

-- The menu rows drawn now (a closed menu's are listed `hidden`).
local function rows(app: any): { string }
	local out = {}
	for line in string.gmatch(app:tree(), "[^\n]+") do
		if string.find(line, " menuitem ", 1, true) and not string.find(line, "hidden", 1, true) then
			table.insert(out, line)
		end
	end
	return out
end

local function shows(app: any, name: string): boolean
	for _, line in rows(app) do
		if string.find(line, item(name) .. " ", 1, true) then
			return true
		end
	end
	return false
end

local function open_now(app: any): boolean
	return #rows(app) > 0
end

local function until_(app: any, done: () -> boolean)
	for _ = 1, 40 do
		if done() then
			return
		end
		app:wait("#no-such-node", { timeout_ms = 50 })
	end
end

local function menu(app: any, target: string)
	app:right_click(target)
	until_(app, function()
		return open_now(app)
	end)
end

local function pick(app: any, name: string)
	app:click(`role:{item(name)}`)
	until_(app, function()
		return not open_now(app)
	end)
	app:frames(5)
end

local function shut(app: any)
	app:key("Escape")
	until_(app, function()
		return not open_now(app)
	end)
end

test("a web link: Open where the setting says, the other place, Copy link", function()
	local app = setup()
	-- "Open links in" Zeron: Open is the panel's, the other place outside.
	menu(app, 'role:link "the docs"')
	expect(shows(app, "Open")).to_be(true)
	expect(shows(app, "Open in default browser")).to_be(true)
	expect(shows(app, "Open in browser panel")).to_be(false)
	expect(shows(app, "Copy link")).to_be(true)
	pick(app, "Copy link")
	expect(app:clipboard()).to_be("https://example.com/a.rs")
	expect(shows(app, "Copy link")).to_be(false)
	menu(app, 'role:link "the docs"')
	pick(app, "Open")
	expect(app:state("calls")).to_contain("panel:https://example.com/a.rs")
	expect(app:state("links_last")).to_be("app:https://example.com/a.rs")
	menu(app, 'role:link "the docs"')
	pick(app, "Open in default browser")
	expect(app:state("links_last")).to_be("system:https://example.com/a.rs")

	-- In the default browser: Open goes outside, the panel's the other place.
	app:set_state("test_open_links", "external")
	app:frames(5)
	menu(app, 'role:link "the docs"')
	expect(shows(app, "Open in browser panel")).to_be(true)
	expect(shows(app, "Open in default browser")).to_be(false)
	pick(app, "Open")
	expect(app:state("links_last")).to_be("system:https://example.com/a.rs")
	menu(app, 'role:link "the docs"')
	local _, before = string.gsub(app:state("calls"), "panel:https://example.com/a.rs", "")
	pick(app, "Open in browser panel")
	local _, after = string.gsub(app:state("calls"), "panel:https://example.com/a.rs", "")
	expect(after).to_be(before + 1)

	-- A prompt's link and an address written bare have the same menu.
	menu(app, 'role:link "the site"')
	expect(shows(app, "Open in browser panel")).to_be(true)
	expect(shows(app, "Copy link")).to_be(true)
	shut(app)
	menu(app, 'role:link "https://github.com/zeronsh/example/pull/1"')
	pick(app, "Copy link")
	expect(app:clipboard()).to_be("https://github.com/zeronsh/example/pull/1")
	expect(app:error()).to_be(nil)
end)

test("a web link keeps its href: the app takes the click, else zgx opens it", function()
	local app = setup()
	-- A link still (its role), whose click the panel took: nothing opened outside.
	expect(app:has('role:link "the docs"')).to_be(true)
	app:click('role:link "the docs"')
	app:frames(5)
	expect(app:state("calls")).to_contain("panel:https://example.com/a.rs")
	expect(#app:opened()).to_be(0)
	-- With links in the default browser, the href opens it.
	app:set_state("test_open_links", "external")
	app:frames(5)
	app:click('role:link "the docs"')
	app:frames(5)
	expect(app:opened()[1]).to_be("https://example.com/a.rs")
	expect(app:state("links_last")).to_be("system:https://example.com/a.rs")
end)

test("external links bypass an internal opener, including pull requests", function()
	local app = setup()
	-- A plugin can claim a link before the shell's browser fallback. The
	-- shared link handler must honor the preference before asking it.
	app:command("set_opener")
	app:set_state("test_open_links", "external")
	app:frames(5)
	local url = "https://github.com/zeronsh/example/pull/1"
	local target = `role:link "{url}"`
	local before = app:state("calls")

	app:click(target)
	app:frames(5)
	expect(app:opened()[1]).to_be(url)
	expect(app:state("calls")).to_be(before)
	expect(app:state("links_last")).to_be(`system:{url}`)

	app:set_state("links_last", "")
	menu(app, target)
	expect(shows(app, "Open in browser panel")).to_be(true)
	pick(app, "Open")
	expect(app:state("calls")).to_be(before)
	expect(app:state("links_last")).to_be(`system:{url}`)

	-- An explicit choice of the panel still works with this preference.
	menu(app, target)
	pick(app, "Open in browser panel")
	expect(app:state("calls")).to_contain(`panel:{url}`)

	-- Switching back lets the internal opener take the same link again.
	app:set_state("test_open_links", "zeron")
	app:frames(5)
	app:click(target)
	app:frames(5)
	expect(app:state("calls")).to_contain(`opener:{url}`)
	expect(#app:opened()).to_be(1)
	expect(app:error()).to_be(nil)
end)

test("a file link: Open, Reveal in Finder on this computer, Copy path", function()
	local app = setup()
	menu(app, 'role:link "the parser"')
	expect(shows(app, "Open")).to_be(true)
	expect(shows(app, "Reveal in Finder")).to_be(true)
	expect(shows(app, "Copy link")).to_be(false)
	pick(app, "Copy path")
	expect(app:clipboard()).to_be("/work/atlas/src/parse.rs")
	menu(app, 'role:link "the parser"')
	pick(app, "Reveal in Finder")
	expect(app:state("calls")).to_contain("reveal:s2:/work/atlas:src/parse.rs")
	menu(app, 'role:link "the parser"')
	pick(app, "Open")
	expect(app:state("calls")).to_contain("open_file:s2:src/parse.rs")
	-- A prompt's file mention, the same.
	menu(app, 'role:link ~"chat.rs"')
	pick(app, "Copy path")
	expect(app:clipboard()).to_be("/work/atlas/src/chat.rs")
	-- Outside the chat's folder: its path to copy, nothing to open.
	menu(app, 'role:link "this one"')
	expect(shows(app, "Copy path")).to_be(true)
	expect(shows(app, "Open")).to_be(false)
	expect(shows(app, "Reveal in Finder")).to_be(false)
	shut(app)
	-- On another device: no Finder to show it in.
	app:command("session_on_box")
	app:frames(10)
	menu(app, 'role:link "the parser"')
	expect(shows(app, "Open")).to_be(true)
	expect(shows(app, "Reveal in Finder")).to_be(false)
	expect(shows(app, "Copy path")).to_be(true)
	expect(app:error()).to_be(nil)
end)

test("a chat mentioned: Open, Copy link (its Zeron link)", function()
	local app = setup()
	menu(app, 'role:link ~"Long chat"')
	expect(shows(app, "Open")).to_be(true)
	expect(shows(app, "Copy path")).to_be(false)
	pick(app, "Copy link")
	expect(app:clipboard()).to_be("zeron://open/chat/s1?workspace=w1")
	local _, before = string.gsub(app:state("calls"), "open:s1", "")
	menu(app, 'role:link ~"Long chat"')
	pick(app, "Open")
	app:frames(20)
	local _, after = string.gsub(app:state("calls"), "open:s1", "")
	expect(after).to_be(before + 1)
	expect(app:error()).to_be(nil)
end)

test("a settings link (About's rows): Open, as its click, and Copy link", function()
	local app = mount("shell_fixture.luau")
	app:resize(1200, 900)
	app:frames(30)
	app:command("settings_stub.sign_in")
	app:command("shell.settings")
	app:frames(20)
	app:click('#settings-nav >> role:tab "About & updates"')
	app:frames(30)
	app:set_state("about", { app_version = "0.2.131", app_build = "1793000000", available = false, engine_version = "0.2.131" })
	app:frames(6)
	app:scroll("#settings-page", 2000)
	app:frames(4)
	menu(app, '#settings-page >> role:button "Website"')
	expect(shows(app, "Open")).to_be(true)
	-- Settings' own links always open outside: no other place to offer.
	expect(shows(app, "Open in browser panel")).to_be(false)
	expect(shows(app, "Open in default browser")).to_be(false)
	pick(app, "Copy link")
	expect(app:clipboard()).to_be("https://zeron.sh")
	menu(app, '#settings-page >> role:button "Release notes"')
	pick(app, "Open")
	local asked = false
	for _, c in (app:state("settings_stub_calls") :: any) or {} do
		asked = asked or c == "open_url:https://github.com/zeronsh/comet/releases/tag/v0.2.131"
	end
	expect(asked).to_be(true)
	expect(app:error()).to_be(nil)
end)
