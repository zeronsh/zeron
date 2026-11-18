-- A store plugin's page (screens/plugins/listing), on the page in
-- ui/plugins_store_fixture.luau, the store answered by ui/store_stub.luau:
-- what it is and who made it; Install, then the consent window as for any
-- plugin (nothing allowed here); Update with its changelog and what it
-- asks more or less, read first; a version Zeron took down, turned back
-- on; one made for a later Zeron; one not in the store; the README's
-- pictures only from its own package (in the lightbox), a picture from
-- the web its link, raw HTML never run; Report.

local function calls(app: any): string
	local ok, list = pcall(function()
		return app:state("store_stub_calls")
	end)
	return if ok and list then table.concat(list, "\n") else ""
end

local function settle(app: any, ms: number?)
	app:wait("#never-there", { timeout_ms = ms or 300 })
	app:frames(10)
end

-- The page of the plugin named `name`, opened from Browse.
local function page(name: string, before: ((app: any) -> ())?): any
	local app = mount("plugins_store_fixture.luau")
	app:resize(1000, 900)
	app:frames(10)
	if before then
		before(app)
	end
	app:click('role:tab "Browse"')
	app:wait("#store-rows", { timeout_ms = 3000 })
	app:frames(10)
	if name ~= "Focus timer" then
		app:fill('#store-card >> role:textbox "Search plugins"', name)
		settle(app, 600)
	end
	app:click(`#store-card >> role:button "{name}"`)
	app:wait("#store-offer", { timeout_ms = 3000 })
	settle(app)
	return app
end

-- About, folded at its height (a long README), opened: its Show more,
-- brought into view and pressed.
local function open_about(app: any)
	local y = tonumber((app:tree(".store-about-fold"):match(" %-?[%d.]+,(%-?[%d.]+) "))) or 0
	app:scroll("#fixture-scroll", y - 400)
	app:frames(5)
	app:click(".store-about-fold")
	app:wait("#never-there", { timeout_ms = 300 })
	app:frames(10)
end

test("a plugin's page says what it is, who made it, which version, how many have it and what it works with", function()
	local app = page("Focus timer")
	local tree = app:tree()
	expect(tree).to_contain('heading "Focus timer"')
	expect(tree).to_contain("by @ada")
	expect(tree).to_contain("Maintainer")
	-- (The store's mark: it has verified the version Install offers.)
	expect(app:tree("#store-header")).to_contain('text "Verified"')
	expect(tree).to_contain("Version 1.2.0 · ")
	expect(tree).to_contain("installs")
	expect(tree).to_contain("This version of Zeron")
	expect(tree).to_contain('link "github.com/ada/focus-timer"')
	expect(tree).to_contain("MIT")
	expect(app:has('#store-offer >> role:button "Install"')).to_be(true)
	expect(app:error()).to_be_nil()
end)

test("what it asks for is said as the consent sheet says it", function()
	local app = page("Focus timer")
	local asks = app:tree("#store-asks")
	expect(asks).to_contain("Add things to Zeron's interface")
	expect(asks).to_contain("Send you notifications")
	expect(asks).to_contain("Connect to these places")
	expect(asks).to_contain("api.example.com")
end)

test("the README is drawn: its own pictures through the store, one from the web as its link, HTML never run", function()
	local app = page("Focus timer")
	local readme = app:tree("#store-readme")
	expect(readme).never.to_contain("**")
	expect(readme).never.to_contain("<script>")
	expect(readme).never.to_contain("zeron.decide")
	-- Its own picture, fetched through the store by its path in the package.
	expect(readme).to_contain('button "View The timer in a chat\'s header"')
	expect(calls(app)).to_contain("picture:focus-timer@1.2.0/shots/header.png")
	-- The web's: a link, never asked for.
	expect(readme).to_contain('link "A picture from the web"')
	expect(calls(app)).never.to_contain("tracker.png")
	expect(readme).to_contain('link "the guide"')
end)

test("the README can't draw Zeron's own chips: a chat, a file, a skill or a picture link is a link's words", function()
	local app = page("Focus timer")
	-- The same words, as links to Zeron's own things and as plain words (a
	-- quote in the README), stand as wide: no chip among them.
	local widths: { number } = {}
	for line in app:tree("#store-readme"):gmatch("[^\n]+") do
		if line:find('"See Your chats, your key, Allow and a picture."', 1, true) then
			table.insert(widths, tonumber(line:match(" ([%d.]+)x[%d.]+$")) or 0)
		end
	end
	expect(#widths).to_be(2)
	expect(widths[1]).to_be(widths[2])
end)

test("its pictures open in the lightbox, one after the other", function()
	local app = page("Focus timer")
	expect(app:has('#store-shots >> role:button "Open header.png"')).to_be(true)
	app:click('#store-shots >> role:button "Open header.png"')
	settle(app)
	expect(app:has('role:dialog')).to_be(true)
	app:key("Escape")
	settle(app)
end)

test("Install asks the engine for the store's version on the device shown; the consent window does the rest", function()
	local app = page("Focus timer")
	app:click('#store-offer >> role:button "Install"')
	app:frames(5)
	expect(calls(app)).to_contain("install:focus-timer@1.2.0")
	-- Still under way: the button busy, nothing installed yet.
	expect(app:tree("#store-offer")).to_contain("Install")
	app:command("store.install_done")
	settle(app)
	-- Waiting for you to allow it (the consent window's): Review, never Allow.
	expect(app:has('#store-offer >> role:button "Review"')).to_be(true)
	expect(app:has('role:button "Allow"')).to_be(false)
	app:click('#store-offer >> role:button "Review"')
	expect(calls(app)).to_contain("review:focus-timer")
	app:command("store.allowed")
	settle(app)
	expect(app:tree("#store-offer")).to_contain("Installed")
end)

test("the head's one slot is the same primary button in every state, its words saying which, where it stands under it", function()
	local app = page("Focus timer")
	local function one(label: string)
		expect(app:count("#store-slot >> role:button")).to_be(1)
		expect(app:count("#store-slot >> .store-action")).to_be(1)
		expect(app:has(`#store-slot >> role:button "{label}"`)).to_be(true)
	end
	local function said(words: string)
		expect(app:tree("#store-state")).to_contain(words)
	end
	-- (The item as the engine would send it next, changed by `change`.)
	local function item(change: (p: any) -> ())
		local now = app:state("plugins")
		local list = {}
		for _, p in now.list do
			local q = table.clone(p)
			if q.id == "focus-timer" then
				change(q)
			end
			table.insert(list, q)
		end
		app:set_state("plugins", { state = now.state, list = list })
		settle(app)
	end
	one("Install")
	expect(app:has("#store-state")).to_be(false)
	app:click('#store-slot >> role:button "Install"')
	app:command("store.install_done")
	settle(app)
	one("Review")
	said("Waiting for you to allow it")
	app:command("store.allowed")
	settle(app)
	-- Installed: said, never a button of its own.
	said("Installed")
	expect(app:count("#store-slot >> role:button") <= 1).to_be(true)
	expect(app:count("#store-slot >> role:button")).to_be(app:count("#store-slot >> .store-action"))
	app:command("store.update_found")
	settle(app)
	one("Update to 1.3.0")
	said("You have 1.2.0")
	item(function(p: any)
		p.update = nil
		p.status = "off"
	end)
	one("Turn on")
	said("Turned off")
end)

test("Show less at the foot of a long note, with versions under it, keeps the control just pressed in view", function()
	local app = page("Focus timer")
	app:set_state("plugins_view", "store:extremes")
	app:wait("#store-header", { timeout_ms = 3000 })
	settle(app)
	local function y_of(sel: string): number
		return tonumber((app:tree(sel):match(" [%d.]+,([%-%d.]+) "))) or 0
	end
	app:scroll("#fixture-scroll", 1e6)
	settle(app)
	-- (Its first page from the detail, the rest read as it's asked for.)
	app:click('role:button "Show all versions"')
	settle(app)
	app:scroll("#fixture-scroll", -1e6)
	settle(app)
	app:scroll("#fixture-scroll", y_of(".store-notes-more[1]") - 450)
	settle(app)
	app:click(".store-notes-more[1]")
	settle(app)
	-- At its foot, the eleven older versions under it.
	app:scroll("#fixture-scroll", y_of(".store-notes-more[1]") - 450)
	settle(app)
	app:click(".store-notes-more[1]")
	settle(app)
	local y = y_of(".store-notes-more[1]")
	expect(y >= 0 and y < 900).to_be(true)
	expect(app:tree(".store-notes-more[1]")).to_contain("Show more")
	expect(app:tree(".store-notes-more[1]")).to_contain("focused")
end)

test("the name's column is one width in every state of the head, at 1000 and at 760", function()
	for _, width in { 1000, 760 } do
		local app = page("Focus timer")
		app:resize(width, 900)
		settle(app)
		local function name_w(): number
			return tonumber((app:tree('#store-header >> role:heading'):match(" [%d.]+,[%d.]+ ([%d.]+)x"))) or 0
		end
		local widths = { name_w() }
		app:click('#store-slot >> role:button "Install"')
		app:command("store.install_done")
		settle(app)
		table.insert(widths, name_w())
		app:command("store.allowed")
		settle(app)
		table.insert(widths, name_w())
		app:command("store.update_found")
		settle(app)
		table.insert(widths, name_w())
		app:command("store.blocked")
		settle(app)
		table.insert(widths, name_w())
		for i = 2, #widths do
			expect(`{width}: {widths[i]}`).to_be(`{width}: {widths[1]}`)
		end
	end
end)

test("in a narrow pane the slot goes under the meta line on the name's edge, the name has the whole row, one width in every state", function()
	local app = page("Focus timer")
	-- (A 496 column: beside the slot the name would have 142, under a row's
	-- least for its words, 180.)
	app:resize(560, 900)
	settle(app)
	local function box(sel: string): { x: number, y: number, w: number, h: number }
		local line = app:tree(sel):match("[^\n]+") or ""
		local x, y, w, h = line:match("(%-?[%d.]+),(%-?[%d.]+) ([%d.]+)x([%d.]+)")
		return { x = tonumber(x) or 0, y = tonumber(y) or 0, w = tonumber(w) or 0, h = tonumber(h) or 0 }
	end
	local header = box("#store-header")
	local widths = {}
	local function check(label: string?)
		expect(app:has(".store-slot-under")).to_be(true)
		local name = box('#store-header >> role:heading')
		local slot = box("#store-slot")
		-- On the name's edge, under the meta line; the name the row's whole.
		expect(math.abs(slot.x - name.x) < 0.6).to_be(true)
		expect(slot.y > name.y + name.h).to_be(true)
		expect(math.abs(name.x + name.w - (header.x + header.w - 17)) < 0.6).to_be(true)
		if label then
			expect(app:has(`#store-slot >> role:button "{label}"`)).to_be(true)
		end
		table.insert(widths, name.w)
	end
	check("Install")
	app:click('#store-slot >> role:button "Install"')
	app:command("store.install_done")
	settle(app)
	check("Review")
	-- (The line beside the button.)
	local review, line = box('#store-slot >> role:button "Review"'), box("#store-state")
	expect(math.abs(line.x - (review.x + review.w + 12)) < 0.6).to_be(true)
	app:command("store.allowed")
	settle(app)
	check(nil)
	app:command("store.update_found")
	settle(app)
	check("Update to 1.3.0")
	for i = 2, #widths do
		expect(widths[i]).to_be(widths[1])
	end
	-- Wider (a 576 column), the slot is back beside the name.
	app:resize(640, 900)
	settle(app)
	expect(app:has(".store-slot-under")).to_be(false)
	expect(app:has(".store-slot-room")).to_be(true)
end)

test("Show more opens a 64 KB note whole in place, and the same control, Show less, folds it back", function()
	local app = page("Focus timer")
	app:set_state("plugins_view", "store:extremes")
	app:wait("#store-header", { timeout_ms = 3000 })
	settle(app)
	app:scroll("#fixture-scroll", 1e6)
	settle(app)
	local more = '.store-notes-more'
	local function notes_h(): number
		return tonumber((app:tree(".store-notes[1]"):match(" [%d.]+,[%-%d.]+ [%d.]+x([%d.]+)"))) or 0
	end
	local folded = notes_h()
	expect(app:tree(`{more}[1]`)).to_contain("Show more")
	app:click(`{more}[1]`)
	settle(app)
	expect(app:tree(`{more}[1]`)).to_contain("Show less")
	expect(notes_h() > 10 * folded).to_be(true)
	-- (Under the whole of it now: the page's foot.)
	app:scroll("#fixture-scroll", 1e6)
	settle(app)
	app:click(`{more}[1]`)
	settle(app)
	expect(app:tree(`{more}[1]`)).to_contain("Show more")
	expect(notes_h()).to_be(folded)
end)

test("an install that fails says why, and Install is there to try again", function()
	local app = page("Focus timer")
	app:click('#store-offer >> role:button "Install"')
	app:frames(5)
	app:command("store.install_failed")
	settle(app)
	expect(app:has('#store-offer >> role:button "Install"')).to_be(true)
	-- In Zeron's words, not the engine's.
	expect(calls(app)).to_contain("toast:What came isn't what the plugin store reviewed")
end)

test("an update shows what's new and what it asks for more or no longer, before Update", function()
	local app = page("Focus timer")
	app:click('#store-offer >> role:button "Install"')
	app:command("store.install_done")
	app:command("store.allowed")
	app:command("store.update_found")
	settle(app)
	expect(app:has('#store-offer >> role:button "Update to 1.3.0"')).to_be(true)
	app:click('#store-offer >> role:button "Update to 1.3.0"')
	settle(app)
	local dialog = app:tree('role:dialog')
	expect(dialog).to_contain("Update Focus timer to 1.3.0?")
	expect(dialog).to_contain("Longer breaks.")
	expect(dialog).to_contain("It asks for more")
	expect(dialog).to_contain("Open links")
	expect(dialog).to_contain("It no longer asks for")
	expect(dialog).to_contain("Connect to")
	expect(calls(app)).never.to_contain("update:focus-timer")
	app:click('role:dialog >> role:button "Update"')
	settle(app)
	expect(calls(app)).to_contain("update:focus-timer")
	expect(app:tree("#store-offer")).to_contain("Installed")
end)

test("a version Zeron took down says why, and can be turned back on", function()
	local app = page("Focus timer")
	app:click('#store-offer >> role:button "Install"')
	app:command("store.install_done")
	app:command("store.allowed")
	app:command("store.blocked")
	settle(app)
	local offer = app:tree("#store-offer")
	expect(offer).to_contain("Zeron turned off Focus timer 1.2.0")
	expect(offer).to_contain("taken down from the store")
	app:click('#store-offer >> role:button "Turn it back on"')
	settle(app)
	-- Asked once more, with why it was taken down.
	expect(calls(app)).never.to_contain("set_off:focus-timer:false")
	expect(app:tree("role:dialog")).to_contain("Turn Focus timer back on?")
	app:click('role:dialog >> role:button "Turn it back on"')
	settle(app)
	-- (Turned on as any plugin is: the engine keeps it on as you chose.)
	expect(calls(app)).to_contain("set_off:focus-timer:false")
	expect(app:tree("#store-offer")).never.to_contain("Zeron turned off")
end)

test("one made for a later Zeron says, in the engine's words, which it needs, and doesn't install", function()
	local app = page("Cost meter")
	local offer = app:tree("#store-offer")
	expect(offer).to_contain("Needs Zeron 9.0.0 or later; this device has 0.4.2.")
	app:click('#store-offer >> role:button "Install"')
	app:frames(5)
	expect(calls(app)).never.to_contain("install:cost-meter")
end)

test("whether it works is what the engine says of this Zeron, never judged here", function()
	local app = page("Focus timer", function(a: any)
		a:command("store_stub.old_zeron")
	end)
	expect(app:tree("#store-offer")).to_contain("Needs Zeron 0.3.0 or later; this device has 0.2.0.")
	expect(app:has('#store-offer >> role:button "Install"')).to_be(true)
	expect(app:tree('#store-offer >> role:button "Install"')).to_contain("disabled")
end)

test("once it's there from the store, the device's pre-releases for it are a switch", function()
	local app = page("Focus timer")
	expect(app:has('role:switch "Try pre-releases"')).to_be(false)
	app:click('#store-offer >> role:button "Install"')
	app:command("store.install_done")
	app:command("store.allowed")
	settle(app)
	expect(app:has('role:switch "Try pre-releases"')).to_be(true)
	local y = tonumber((app:tree('role:switch "Try pre-releases"'):match(" %-?[%d.]+,(%-?[%d.]+) "))) or 0
	app:scroll("#fixture-scroll", y - 400)
	app:frames(10)
	app:click('role:switch "Try pre-releases"')
	settle(app)
	expect(calls(app)).to_contain("prereleases:focus-timer:true")
end)

test("a keyword on the page narrows Browse to it", function()
	local app = page("Focus timer")
	app:click('#store-keywords >> role:button "pomodoro"')
	app:wait("#store-card", { timeout_ms = 3000 })
	settle(app)
	expect(calls(app)).to_contain("keyword=pomodoro")
	expect(app:has('#store-card >> role:button "Remove pomodoro"')).to_be(true)
end)

test("Report sends what's wrong to Zeron's reviewers", function()
	local app = page("Focus timer")
	app:scroll("#fixture-scroll", 3000)
	app:frames(10)
	app:click('role:link "Report this plugin"')
	settle(app)
	expect(app:tree('role:dialog')).to_contain("Report Focus timer")
	app:type('role:dialog >> role:textbox "More about it"', "it crashed")
	app:click('role:dialog >> role:button "Send report"')
	settle(app)
	expect(calls(app)).to_contain("report:focus-timer:broken:it crashed")
end)

test("a page opened again draws at once from what was read, and is read again behind it", function()
	local app = page("Focus timer")
	app:click('role:button "Back to Plugins"')
	app:wait("#store-card", { timeout_ms = 3000 })
	app:frames(10)
	-- (Read once more as it opens again: Browse's own reads, a featured
	-- card's picture among them, aren't the page's.)
	local _, before = calls(app):gsub("detail:focus%-timer", "")
	app:command("store_stub.hold")
	app:click('#store-card >> role:button "Focus timer"')
	app:frames(30)
	expect(app:has("#store-offer")).to_be(true)
	expect(app:has("#store-loading")).to_be(false)
	app:command("store_stub.release")
	settle(app)
	local _, n = calls(app):gsub("detail:focus%-timer", "")
	expect(n - before).to_be(1)
end)

test("a page the store doesn't have says so, with a way back to Browse", function()
	local app = mount("plugins_store_fixture.luau")
	app:resize(1000, 900)
	app:frames(10)
	app:set_state("plugins_view", "store:no-such-plugin")
	app:wait('"This plugin isn\'t in the store"', { timeout_ms = 3000 })
	app:frames(20)
	app:click('role:button "Browse plugins"')
	app:wait("#store-card", { timeout_ms = 3000 })
end)

test("a page that can't be read offline says so, and Try again reads it", function()
	local app = mount("plugins_store_fixture.luau")
	app:resize(1000, 900)
	app:frames(10)
	app:command("store_stub.offline")
	app:set_state("plugins_view", "store:focus-timer")
	app:wait('"Can\'t reach the plugin store"', { timeout_ms = 3000 })
	app:frames(20)
	app:command("store_stub.online")
	app:click('role:button "Try again"')
	app:wait("#store-offer", { timeout_ms = 3000 })
end)

test("another plugin with the same id, from elsewhere, is never taken for this one", function()
	local app = page("Focus timer")
	app:set_state("plugins", (function()
		local now = app:state("plugins")
		local list = {}
		for _, p in now.list do
			table.insert(list, p)
		end
		table.insert(list, { id = "focus-timer", name = "Focus timer", version = "9.9.9", author = { kind = "unknown" }, status = "on", permissions = {}, allowed = {}, ask = {}, hash = "x", source = { kind = "link", url = "https://evil.example/f.zeron-plugin" } })
		return { state = now.state, list = list }
	end)())
	settle(app)
	local offer = app:tree("#store-offer")
	expect(offer).to_contain("different plugin with this id")
	expect(app:tree('#store-offer >> role:button "Install"')).to_contain("disabled")
	expect(app:has('#store-offer >> role:button "Turn on"')).to_be(false)
end)

test("a README with more pictures than a page shows asks for only so many", function()
	local app = page("Screenshot tray")
	app:scroll("#fixture-scroll", 20000)
	settle(app, 500)
	local _, n = calls(app):gsub("picture:screenshot%-tray", "")
	-- (Forty in its README, every one drawn: twenty-four asked, the rest links.)
	expect(n).to_be(24)
	expect(app:error()).to_be_nil()
end)

test("a listing at every limit at once lays out on the page, its name and words kept inside it", function()
	local app = mount("plugins_store_fixture.luau")
	app:resize(1000, 900)
	app:frames(10)
	app:set_state("plugins_view", "store:extremes")
	app:wait("#store-readme", { timeout_ms = 10000 })
	settle(app, 500)
	expect(app:error()).to_be_nil()
	-- Nothing wider than the page (its measure), however long its words.
	for _, id in { "#store-offer", "#store-readme", "#store-keywords", "#store-shots" } do
		local w = tonumber((app:tree(id):match("^%s*@%d+[^\n]- %d+,%-?%d+ (%d+)x"))) or 0
		expect(`{id} {w <= 640}`).to_be(`{id} true`)
	end
	expect(app:tree()).to_contain("Maintainer")
end)

test("an update from a new publisher says so by name, and one still in review says only your devices get it", function()
	local app = page("Focus timer")
	app:click('#store-offer >> role:button "Install"')
	app:command("store.install_done")
	app:command("store.allowed")
	app:command("store.update_new_owner")
	settle(app)
	app:click('#store-offer >> role:button "Update to 1.3.0"')
	settle(app)
	local dialog = app:tree("role:dialog")
	expect(dialog).to_contain("It has a new publisher, @ada.")
	expect(dialog).to_contain("still in review")
end)

test("an update signed with a different key says so above its notes, and Update still acts", function()
	local app = page("Focus timer")
	app:click('#store-offer >> role:button "Install"')
	app:command("store.install_done")
	app:command("store.allowed")
	app:command("store.update_new_key")
	settle(app)
	app:click('#store-offer >> role:button "Update to 1.3.0"')
	settle(app)
	local dialog = app:tree("role:dialog")
	-- (The tree shortens a long text: its start.)
	local at, notes = dialog:find("This version is signed with a different key", 1, true), dialog:find('text "What\'s new"', 1, true)
	expect(at ~= nil and notes ~= nil and at < notes).to_be(true)
	expect(app:tree('role:dialog >> role:button "Update"')).never.to_contain("disabled")
	app:click('role:dialog >> role:button "Update"')
	settle(app)
	expect(calls(app)).to_contain("update:focus-timer")
end)

test("a README of the store's whole 512 KB opens with its first piece, the rest a piece at a time", function()
	local app = mount("plugins_store_fixture.luau")
	app:resize(1000, 900)
	app:frames(10)
	app:set_state("plugins_view", "store:extremes")
	app:wait("#store-readme", { timeout_ms = 10000 })
	settle(app, 300)
	expect(app:error()).to_be_nil()
	-- (Folded at its height first: opened, its first piece whole.)
	open_about(app)
	expect(app:has("#store-readme-more")).to_be(true)
	local function height(): number
		return tonumber((app:tree("#store-readme"):match("^%s*@%d+[^\n]- [%d.]+x([%d.]+)"))) or 0
	end
	local before = height()
	local y = tonumber((app:tree("#store-readme-more"):match(" %-?[%d.]+,(%-?[%d.]+) "))) or 0
	app:scroll("#fixture-scroll", y - 400)
	app:frames(10)
	app:click('#store-readme-more >> role:button "Show more"')
	settle(app, 300)
	expect(height() > before + 1000).to_be(true)
	expect(app:error()).to_be_nil()
end)

-- About folded at its height (screens/plugins/listing ABOUT_FOLD, 1,200,
-- with a slack of 240), on the page opened with `readme` as Focus timer's.
local function about_page(readme: string): any
	local app = mount("plugins_store_fixture.luau")
	app:resize(1000, 900)
	app:frames(10)
	app:set_state("store_stub_readme", readme)
	app:set_state("plugins_view", "store:focus-timer")
	app:wait("#store-readme", { timeout_ms = 5000 })
	settle(app, 400)
	return app
end
local function box(app: any, selector: string): (number, number)
	local y, h = app:tree(selector):match("^%s*@%d+[^\n]- %-?[%d.]+,(%-?[%d.]+) [%d.]+x([%d.]+)")
	return tonumber(y) or 0, tonumber(h) or 0
end

test("a README built to cost folds at its height, under What it asks for, and Report stays near the top", function()
	local app = about_page(string.sub(string.rep("Heading\n=======\n\n", 32768), 1, 512 * 1024))
	expect(app:error()).to_be_nil()
	-- What a person weighs first comes before the author's words.
	local asks_y = box(app, 'role:heading "What it asks for"')
	local about_y = box(app, 'role:heading "About"')
	expect(asks_y < about_y).to_be(true)
	-- Folded: no taller than the fold and its control, its next pieces
	-- not offered until it's open.
	local _, card_h = box(app, "#store-readme")
	expect(card_h <= 1200 + 80).to_be(true)
	expect(app:has(".store-about-fold")).to_be(true)
	expect(app:has("#store-readme-more")).to_be(false)
	-- Report, the door a page like this needs, is within 2,700 of the top
	-- on first open (2,576 measured at 1000 wide), not 36,000 down.
	local report_y = box(app, 'role:link "Report this plugin"')
	expect(report_y > about_y and report_y <= 2700).to_be(true)
end)

test("a short README isn't folded, nor one just over the fold by less than its slack", function()
	-- Focus timer's own (about 670 high).
	local app = about_page("")
	expect(app:has(".store-about-fold")).to_be(false)
	-- A list item is 30 high: 44 of them about 1,320, past the fold by
	-- less than 240, shown whole; 52, about 1,560, folded.
	app = about_page(string.rep("- item\n", 44))
	local _, h = box(app, "#store-readme")
	expect(h > 1200 + 32).to_be(true)
	expect(app:has(".store-about-fold")).to_be(false)
	app = about_page(string.rep("- item\n", 52))
	expect(app:has(".store-about-fold")).to_be(true)
end)

test("About's Show more, then Show less, comes back to the same height with its control in view", function()
	local app = about_page(string.rep("A line of a long README.\n\n", 400))
	local _, folded_h = box(app, "#store-readme")
	open_about(app)
	local _, open_h = box(app, "#store-readme")
	expect(open_h > folded_h + 1000).to_be(true)
	expect(app:tree(".store-about-fold")).to_contain("Show less")
	-- Far down the open text, in a window short enough that the page's end
	-- alone wouldn't show it, then folded again.
	app:resize(1000, 500)
	app:frames(5)
	local y = box(app, ".store-about-fold")
	app:scroll("#fixture-scroll", y - 300)
	app:frames(5)
	app:click(".store-about-fold")
	settle(app, 300)
	local _, again_h = box(app, "#store-readme")
	expect(math.abs(again_h - folded_h) < 0.5).to_be(true)
	local at, at_h = box(app, ".store-about-fold")
	expect(`{at}`).to_be(`{math.clamp(at, 0, 500 - at_h)}`)
	expect(app:tree(".store-about-fold")).to_contain("Show more")
end)

test("a version the store hasn't verified says so plainly under the summary, offering the newest verified one", function()
	-- Todo sweep: 1.2.0 not verified, 1.0.0 verified.
	local app = page("Todo sweep")
	local line = app:tree("#store-verified-line")
	expect(line).to_contain("1.2.0 isn't verified yet. 1.0.0 is.")
	expect(app:has("#store-header >> .store-verified")).to_be(false)
	app:click('#store-verified-line >> role:link "Install 1.0.0 instead"')
	settle(app)
	expect(calls(app)).to_contain("install:todo-sweep@1.0.0")
	-- Sentry traces: none verified, nothing to offer.
	app = page("Sentry traces")
	expect(app:tree("#store-verified-line")).to_contain("No version of this plugin is verified yet.")
	expect(app:has('#store-verified-line >> role:link')).to_be(false)
	-- Focus timer: verified, no line.
	app = page("Focus timer")
	expect(app:has("#store-verified-line")).to_be(false)
end)

test("each version says whether the store has verified it, and any listed one installs from its row; a stopped one doesn't", function()
	local app = page("Focus timer")
	app:scroll("#fixture-scroll", 1e6)
	settle(app)
	local versions = app:tree('role:group "Versions"')
	expect(versions).to_contain("Verified on")
	expect(versions).to_contain("No new installs")
	-- 1.1.0 was stopped: no Install of its own.
	expect(app:count('role:group "Versions" >> role:button "Install"')).to_be(2)
	app:click('role:group "Versions" >> role:button "Install"[2]')
	settle(app)
	expect(calls(app)).to_contain("install:focus-timer@1.0.0")
end)

test("a plugin's versions past the first page are read as Show all asks for them, every one then listed", function()
	local app = page("Focus timer")
	app:set_state("plugins_view", "store:extremes")
	app:wait("#store-header", { timeout_ms = 3000 })
	settle(app)
	app:scroll("#fixture-scroll", 1e6)
	settle(app)
	expect(calls(app)).never.to_contain("versions:extremes")
	app:click('role:button "Show all versions"')
	settle(app, 600)
	expect(calls(app)).to_contain("versions:extremes:5")
	expect(app:count(".store-version")).to_be(12)
	expect(app:has('role:button "Show all versions"')).to_be(false)
end)
