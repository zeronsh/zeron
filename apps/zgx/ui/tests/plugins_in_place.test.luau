-- Settings → Plugins follows the engine's list in place: a plugin arriving,
-- a new version, one turned off, edited after it was allowed, or failing
-- to load leaves every row (and its own) the same node, and swaps only
-- what's at its end. The switch, Remove and New plugin ask the host. On
-- the page in ui/plugins_fixture.luau.

local function node(app: any, kind: string, label: string): string?
	for line in app:tree():gmatch("[^\n]+") do
		if line:find(` {kind} "{label}"`, 1, true) then
			return line:match("@(%d+)")
		end
	end
	return nil
end

-- A row's node: the row is the Settings group child holding the name.
local function row(app: any, name: string): string?
	for line in app:tree():gmatch("[^\n]+") do
		if line:find(`"{name}"`, 1, true) and line:find("text", 1, true) then
			return line:match("@(%d+)")
		end
	end
	return nil
end

local function open(): any
	local app = mount("plugins_fixture.luau")
	app:resize(1000, 900)
	app:frames(20)
	return app
end

test("each plugin shows who made it and what it can do now", function()
	local app = open()
	local tree = app:tree()
	expect(tree).to_contain("Compact sidebar")
	expect(tree).to_contain("by you")
	expect(tree).to_contain("by @ada")
	expect(app:has('role:switch "Focus"')).to_be(true)
	expect(app:has('role:button "Review"')).to_be(false)
end)

test("a plugin arriving, a new version and a status change keep every row", function()
	local app = open()
	local compact = row(app, "Compact sidebar")
	local focus = row(app, "Focus")
	local focus_switch = node(app, "switch", "Focus")
	expect(compact).never.to_be_nil()
	expect(focus).never.to_be_nil()

	-- One dropped in: it waits for you, the rest stay.
	app:command("arrive")
	app:frames(20)
	expect(app:has('role:button "Review"')).to_be(true)
	expect(app:tree()).to_contain("Unknown author")
	expect(`compact @{row(app, "Compact sidebar")}`).to_be(`compact @{compact}`)
	expect(`focus @{row(app, "Focus")}`).to_be(`focus @{focus}`)
	expect(`switch @{node(app, "switch", "Focus")}`).to_be(`switch @{focus_switch}`)

	-- A new version: its badge, in place.
	app:command("bump")
	app:frames(20)
	expect(app:tree()).to_contain("1.1.0")
	expect(`focus @{row(app, "Focus")}`).to_be(`focus @{focus}`)
	expect(`switch @{node(app, "switch", "Focus")}`).to_be(`switch @{focus_switch}`)

	-- Edited after it was allowed: Review where its switch was, the row the same.
	app:command("tamper")
	app:frames(20)
	expect(app:has('role:switch "Focus"')).to_be(false)
	expect(app:tree()).to_contain("Changed since you allowed it")
	expect(`focus @{row(app, "Focus")}`).to_be(`focus @{focus}`)
	expect(`compact @{row(app, "Compact sidebar")}`).to_be(`compact @{compact}`)

	-- One that can't load says why, and offers its folder.
	app:command("break")
	app:frames(20)
	expect(app:tree()).to_contain("permission Zeron doesn")
	expect(app:has('role:button "Show folder"')).to_be(true)
end)

test("the switch, Review and Remove ask the host", function()
	local app = open()
	-- (Asked once the knob has landed: time passes in a wait, not frames.)
	local function landed()
		app:wait("#the-knob-lands", { timeout_ms = 400 })
		app:frames(10)
	end
	app:click('role:switch "Focus"')
	app:frames(2)
	expect(app:state("calls") or "").never.to_contain("set_off:focus:true")
	landed()
	expect(app:state("calls")).to_contain("set_off:focus:true")
	app:click('role:switch "Focus"')
	landed()
	expect(app:state("calls")).to_contain("set_off:focus:false")

	app:command("arrive")
	app:frames(20)
	app:click('role:button "Review"')
	expect(app:state("calls")).to_contain("review:ping")

	app:click('role:button "Remove Ping"')
	app:frames(20)
	app:click('role:button "Remove"')
	app:frames(20)
	expect(app:state("calls")).to_contain("remove:ping")
	expect(app:has('role:button "Review"')).to_be(false)
end)

test("search narrows the list and Yours holds only yours", function()
	local app = open()
	app:type('role:textbox "Search plugins"', "focus")
	app:frames(20)
	expect(app:has('role:switch "Focus"')).to_be(true)
	expect(app:has('role:switch "Compact sidebar"')).to_be(false)
	app:fill('role:textbox "Search plugins"', "nothing like it")
	app:frames(20)
	expect(app:tree()).to_contain("Nothing matches")
	app:fill('role:textbox "Search plugins"', "")
	app:frames(20)
	app:click('role:tab "Yours"')
	app:frames(20)
	expect(app:has('role:switch "Compact sidebar"')).to_be(true)
	expect(app:has('role:switch "Focus"')).to_be(false)
end)

-- New plugin and Install… are off the page for now: their tests are
-- kept, not run, until they come back (`later` → `test`).
local function later(_name: string, _body: () -> ()) end

later("with none, it says how to get one, and New plugin makes one of yours", function()
	local app = open()
	app:command("empty")
	app:frames(20)
	expect(app:tree()).to_contain("No plugins yet")
	app:click('"New plugin"[1]')
	app:frames(20)
	app:type('role:textbox "Name"', "My plugin")
	app:click('role:button "Make it"')
	app:frames(20)
	expect(app:state("calls")).to_contain("new:My plugin")
	expect(app:has('role:switch "My plugin"')).to_be(true)
end)

-- (No "Not shown in this template" line: it misfired on places that exist
-- only for a moment, a hover card's, and said nothing a person could act
-- on. The row keeps its description.)
test("a plugin aiming at an element the template in use dropped keeps its description", function()
	local app = open()
	app:command("hidden")
	app:frames(20)
	expect(app:tree()).never.to_contain("Not shown in this template")
end)

test("a plugin placed where what it needs isn't says so on its row", function()
	local app = open()
	app:command("problem")
	app:frames(20)
	expect(app:tree()).to_contain("Focus needs a session, but it's in the title bar")
end)

test("the list changing under an open page asks nothing of the host", function()
	local app = open()
	-- Every way the engine's list moves while the page is open: one
	-- arriving, a new version, one allowed or declined in its window, one
	-- turned off and on elsewhere, one edited after it was allowed, one
	-- failing to load, one removed.
	for _, change in { "arrive", "bump", "allowed", "declined", "engine_off", "engine_on", "engine_off", "tamper", "break" } do
		app:command(change)
		app:frames(10)
	end
	-- Nothing was decided, turned on or off, reviewed or removed.
	expect(app:state("calls")).to_be("")
	-- And what shows follows the list: declined asks for review again.
	expect(app:has('role:button "Review"')).to_be(true)
end)

test("a switch follows the engine's status without asking it", function()
	local app = open()
	local function checked(): boolean
		for line in app:tree():gmatch("[^\n]+") do
			if line:find('switch "Focus"', 1, true) then
				return line:find(" checked", 1, true) ~= nil
			end
		end
		return false
	end
	expect(checked()).to_be(true)
	app:command("engine_off")
	app:frames(10)
	expect(checked()).to_be(false)
	app:command("engine_on")
	app:frames(10)
	expect(checked()).to_be(true)
	expect(app:state("calls")).to_be("")
end)

test("with none installed, a search says nothing matches, and clearing it says how to get one", function()
	local app = open()
	app:command("empty")
	app:frames(20)
	expect(app:tree()).to_contain("No plugins yet")
	app:type('role:textbox "Search plugins"', "ping")
	app:frames(20)
	expect(app:tree()).to_contain("Nothing matches")
	expect(app:tree()).never.to_contain("No plugins yet")
	app:fill('role:textbox "Search plugins"', "")
	app:frames(20)
	expect(app:tree()).to_contain("No plugins yet")
end)

test("Zeron's own says so, shows what it can do, and can be turned off, not removed", function()
	local app = open()
	app:command("builtin")
	app:frames(20)
	local tree = app:tree()
	expect(tree).to_contain("by Zeron")
	-- (What it can do on a line of its own under the description, whole.)
	expect(tree).to_contain("Your sessions' pull requests.")
	expect(tree).to_contain('"by Zeron"')
	expect(tree).to_contain('text "It can see your pull')
	expect(app:has('role:switch "GitHub"')).to_be(true)
	expect(app:has('role:button "Remove GitHub"')).to_be(false)
	expect(app:has('role:button "Remove Focus"')).to_be(true)
end)

test("narrow, one of Zeron's own still shows all it's allowed, wrapped under its description", function()
	local app = open()
	app:command("builtin")
	app:frames(20)
	app:resize(560, 900)
	app:frames(20)
	local line = string.match(app:tree(), '[^\n]*text "It can see your pull[^\n]*') or ""
	-- Wrapped past one line, not faded at the row's end.
	local h = tonumber(string.match(line, " %-?%d+,%-?%d+ %d+x(%d+)")) or 0
	expect(h > 20).to_be(true)
end)

test("the page checks sources as it opens, and a found update says what it asks and updates", function()
	local app = open()
	expect(app:state("checks")).to_be(1)
	expect(app:has('role:button "Update"')).to_be(false)
	local focus = row(app, "Focus")
	app:command("update_found")
	app:frames(20)
	expect(app:tree()).to_contain("Update available: 2.0.0.")
	expect(app:tree()).to_contain("It also asks to open links")
	expect(row(app, "Focus")).to_be(focus)
	app:click('role:button "Update"')
	app:frames(20)
	expect(app:state("calls")).to_contain("update:focus")
	expect(app:has('role:button "Update"')).to_be(false)
	expect(app:tree()).never.to_contain("Update available")
	expect(app:tree()).to_contain("2.0.0")
end)

test("an update signed by someone else says so and offers nothing to press", function()
	local app = open()
	app:command("update_refused")
	app:frames(20)
	expect(app:tree()).to_contain("Version 6.6.6 wasn't installed: The new version is signed")
	expect(app:tree()).never.to_contain("new_signer")
	expect(app:has('role:button "Update"')).to_be(false)
end)

test("Install takes a link and the plugin arrives waiting for Review", function()
	local app = open()
	app:click('role:button "Install"')
	app:frames(20)
	expect(app:tree()).to_contain("Install a plugin")
	expect(app:has('role:button "Choose File"')).to_be(true)
	app:type('role:textbox "Link"', "ada/clock")
	app:click('role:dialog >> role:button "Install"')
	app:frames(20)
	expect(app:state("calls")).to_contain("install:ada/clock")
	-- On its way: the dialog stays, its Install working.
	expect(app:state("plugins_installing")).to_be(true)
	expect(app:state("plugins_install_busy")).to_be(true)
	app:command("installed")
	app:frames(20)
	expect(app:state("plugins_installing")).to_be(false)
	expect(app:has('role:button "Review"')).to_be(true)
end)

test("an install that fails keeps the dialog, to try again", function()
	local app = open()
	app:click('role:button "Install"')
	app:frames(20)
	app:type('role:textbox "Link"', "ada/nothing")
	app:click('role:dialog >> role:button "Install"')
	app:frames(10)
	app:command("install_failed")
	app:frames(20)
	-- (Why, in a toast; the dialog kept, its link as typed.)
	expect(app:state("plugins_installing")).to_be(true)
	expect(app:state("plugins_install_busy")).to_be(false)
	expect(app:has('role:dialog >> role:button "Install"')).to_be(true)
	-- GitHub turning requests away says so, in the locale's words.
	expect(app:state("calls")).to_contain("toast:GitHub is turning away requests from this device for now. Try again later")
	expect(app:state("calls")).never.to_contain("rate_limited")
end)

test("one Zeron can't vouch for says Unknown author by its version, and where it came from", function()
	local app = open()
	-- (The badges shown: a giving row's copies laid out only to be
	-- measured are hidden.)
	local function badges(): number
		local n = 0
		for line in string.gmatch(app:tree(), "[^\n]+") do
			if string.find(line, '"Unknown author"', 1, true) and not string.find(line, " hidden", 1, true) then
				n += 1
			end
		end
		return n
	end
	-- Zeron's own, yours and a verified author's: never.
	app:command("builtin")
	app:frames(20)
	expect(badges()).to_be(0)
	-- Someone Zeron can't vouch for, from nowhere named: its badge says it,
	-- once; no byline saying it again.
	app:command("arrive")
	app:frames(20)
	expect(badges()).to_be(1)
	local function card_h(): number
		return tonumber((app:tree("#catalog-card"):match("^%s*@%d+ #catalog%-card %-?[%d.]+,%-?[%d.]+ [%d.]+x([%d.]+)"))) or 0
	end
	local function y_of(words: string): number
		return tonumber((app:tree("#catalog-card"):match(`text "{words}[^"]*" %-?[%d.]+,(%-?[%d.]+)`))) or 0
	end
	local bare = card_h()
	-- (Its next line a name's step under its name, as its neighbours'.)
	expect(y_of("Waiting for you") - y_of("Ping")).to_be(20)
	-- Where it came from, once known: its byline, the badge kept.
	app:command("ping_from_github")
	app:frames(20)
	expect(app:tree()).to_contain("From github.com/eve/ping")
	expect(badges()).to_be(1)
	expect(y_of("From github") - y_of("Ping")).to_be(20)
	-- (The line and its step alone come and go: 16 and 2.)
	expect(card_h() - bare).to_be(18)
end)


-- While the account's setup isn't finished the engine answers no plugin
-- call: the page says to finish it, and the list arrives once it is (the
-- host watches again, src/engine/rpc.rs `watch_set_up`), no relaunch.
test("with the account's setup unfinished it says so, and the plugins load once it's done", function()
	local app = open()
	app:command("setup_incomplete")
	app:frames(20)
	local tree = app:tree()
	expect(tree).to_contain("Finish setting up your account")
	expect(tree).never.to_contain("Compact sidebar")
	app:command("set_up")
	app:frames(20)
	tree = app:tree()
	expect(tree).never.to_contain("Finish setting up your account")
	expect(tree).to_contain("Compact sidebar")
end)

-- Installed's Update is drawn as it always was (its own button, its words
-- underlined), not the store's inline link, and the keys reach it.
test("Update draws as it always did, and the keys reach it", function()
	local app = open()
	app:command("update_found")
	app:frames(20)
	expect(app:has('role:button "Update"')).to_be(true)
	expect(app:has(".settings-inline-link")).to_be(false)
	app:click('role:textbox "Search plugins"')
	app:frames(5)
	local reached = false
	for _ = 1, 30 do
		app:key("Tab")
		app:frames(3)
		if app:tree('role:button "Update"'):find("focused", 1, true) then
			reached = true
			break
		end
	end
	expect(reached).to_be(true)
end)

-- The tabs' action (Install…) stays inside the page at every width: beside
-- the tabs where they and it fit (as always, at 1000 and wider), else on a
-- line of its own under them, ending on the same edge, still pressable.
test("Install stays inside the page beside the tabs, or under them where they don't fit", function()
	local placed: { [number]: boolean } = {}
	for _, width in { 400, 560, 760, 1000 } do
		local app = mount("plugins_fixture.luau")
		app:resize(width, 900)
		app:frames(30)
		local tree = app:tree()
		local cx, _, cw = tree:match("#catalog%-card (%-?[%d.]+),(%-?[%d.]+) ([%d.]+)x")
		local tx, ty, tw, th = tree:match('tablist "Plugins" (%-?[%d.]+),(%-?[%d.]+) ([%d.]+)x([%d.]+)')
		-- (The one that shows: beside the tabs, or on its own line.)
		local ix, iy, iw
		local shown = 0
		for line in tree:gmatch('[^\n]*button "Install"[^\n]*') do
			if not line:find("hidden", 1, true) then
				shown += 1
				ix, iy, iw = line:match('button "Install" (%-?[%d.]+),(%-?[%d.]+) ([%d.]+)x')
			end
		end
		expect(`{width} shown={shown}`).to_be(`{width} shown=1`)
		assert(cx and tx and ix, `{width}: {tree}`)
		local right = tonumber(cx) + tonumber(cw)
		expect(`{width} inside={tonumber(ix) + tonumber(iw) <= right + 0.5}`).to_be(`{width} inside=true`)
		local beside = math.abs(tonumber(iy) - tonumber(ty)) < tonumber(th)
		placed[width] = beside
		if width == 1000 then
			-- (As it always was: beside the tabs, ending on the card's words.)
			expect(`{width} beside={beside}`).to_be(`{width} beside=true`)
		elseif not beside then
			-- (Under the rule, the catalog's own step below it: 16.)
			expect(tonumber(iy) - (tonumber(ty) + tonumber(th))).to_be(16)
		end
		-- (Where it fits, the head is as it always was: the card one tab
		-- row and the step under the tabs' top.)
		local _, cy = tree:match("#catalog%-card (%-?[%d.]+),(%-?[%d.]+)")
		if beside then
			expect(`{width} head={tonumber(cy) - tonumber(ty)}`).to_be(`{width} head=52`)
		end
		app:click('role:button "Install"')
		app:frames(10)
		expect(app:has('role:dialog')).to_be(true)
	end
	-- (Narrow enough, it does go under: the give-way is exercised.)
	expect(placed[400]).to_be(false)
end)

-- Templates' catalog, where its tabs and Install fit at 560 and 1000, is as
-- it always was: Install beside the tabs, the card where it was.
test("Templates' head where its action fits is as it always was", function()
	for _, width in { 560, 1000 } do
		local app = mount("templates_fixture.luau")
		app:resize(width, 900)
		app:frames(30)
		local tree = app:tree()
		local _, ty, _, th = tree:match('tablist "Templates" (%-?[%d.]+),(%-?[%d.]+) ([%d.]+)x([%d.]+)')
		local _, cy = tree:match("#catalog%-card (%-?[%d.]+),(%-?[%d.]+)")
		local iy: string? = nil
		for line in tree:gmatch('[^\n]*button "Install"[^\n]*') do
			if not line:find("hidden", 1, true) then
				iy = line:match('button "Install" %-?[%d.]+,(%-?[%d.]+)')
			end
		end
		assert(ty and cy and iy, `{width}: {tree}`)
		expect(`{width} beside={math.abs(tonumber(iy) - tonumber(ty)) < tonumber(th)}`).to_be(`{width} beside=true`)
		expect(`{width} head={tonumber(cy) - tonumber(ty)}`).to_be(`{width} head=52`)
	end
end)

-- At every width from 400 to 1100, a catalog's action is whole and inside
-- its card: beside the tabs ending on the line's content edge (never
-- clipped), or on its own line one step under the rule. Plugins' Install
-- and Themes' Import theme (longer: Themes' tabs alone run wide), the
-- decision taken from the action's own width against the room it got,
-- whatever the font measures.
test("a catalog's action is whole and inside its card at every width from 400 to 1100", function()
	for _, case in { { fx = "plugins_fixture.luau", tabs = "Plugins", action = "Install" }, { fx = "themes_fixture.luau", cmd = "themes", tabs = "Themes", action = "Import" } } do
		for width = 400, 1100, 20 do
			local app = mount(case.fx)
			app:resize(width, 900)
			app:frames(10)
			if case.cmd then
				app:command(case.cmd)
			end
			app:frames(30)
			local tree = app:tree()
			local cx, _, cw = tree:match("#catalog%-card (%-?[%d.]+),(%-?[%d.]+) ([%d.]+)x")
			local _, ty, _, th = tree:match(`tablist "{case.tabs}" (%-?[%d.]+),(%-?[%d.]+) ([%d.]+)x([%d.]+)`)
			local ax, ay, aw
			local shown = 0
			for line in tree:gmatch(`[^\n]*button "{case.action}[^\n]*`) do
				if not line:find("hidden", 1, true) then
					shown += 1
					ax, ay, aw = line:match(' (%-?[%d.]+),(%-?[%d.]+) ([%d.]+)x')
				end
			end
			local at = `{case.tabs} {width}`
			assert(cx and ty and ax, `{at}: {tree}`)
			expect(`{at} shown={shown}`).to_be(`{at} shown=1`)
			local right = tonumber(cx) + tonumber(cw)
			local edge = tonumber(ax) + tonumber(aw)
			local beside = math.abs(tonumber(ay) - tonumber(ty)) < tonumber(th)
			-- (Whole: ending on or before the line's content edge, the card's
			-- words' inset in from its side.)
			expect(`{at} whole={edge <= right - 12}`).to_be(`{at} whole=true`)
			if not beside then
				expect(`{at} step={tonumber(ay) - (tonumber(ty) + tonumber(th))}`).to_be(`{at} step=16`)
			end
		end
	end
end)

-- Widened step by step in one window (not built fresh at each width), the
-- action stays whole and inside its card at every step: under the rule
-- until it truly fits, then beside the tabs, once, never back and forth.
-- Themes' tabs alone run past the page at the narrow end, where a guess
-- from the shortfall would bring it back beside too soon.
test("widened step by step, a catalog's action moves beside its tabs once, only when it fits", function()
	for _, case in { { fx = "themes_fixture.luau", cmd = "themes", tabs = "Themes", action = "Import" }, { fx = "plugins_fixture.luau", tabs = "Plugins", action = "Install" } } do
		-- (From narrower than the tabs themselves, whatever the font: the
		-- case where the action's need can't be read off its shortfall.)
		local app = mount(case.fx)
		app:resize(260, 900)
		app:frames(10)
		if case.cmd then
			app:command(case.cmd)
		end
		app:frames(30)
		local switches, was_beside = 0, nil
		local overflowed = false
		for width = 260, 1100, 20 do
			app:resize(width, 900)
			app:frames(20)
			local tree = app:tree()
			local cx, _, cw = tree:match("#catalog%-card (%-?[%d.]+),(%-?[%d.]+) ([%d.]+)x")
			local _, ty, _, th = tree:match(`tablist "{case.tabs}" (%-?[%d.]+),(%-?[%d.]+) ([%d.]+)x([%d.]+)`)
			local ax, ay, aw
			for line in tree:gmatch(`[^\n]*button "{case.action}[^\n]*`) do
				if not line:find("hidden", 1, true) then
					ax, ay, aw = line:match(' (%-?[%d.]+),(%-?[%d.]+) ([%d.]+)x')
				end
			end
			local at = `{case.tabs} {width}`
			assert(cx and ty and ax, `{at}: {tree}`)
			-- (Whether the tabs alone run past the card here.)
			local tx, _, tw = tree:match(`tablist "{case.tabs}" (%-?[%d.]+),(%-?[%d.]+) ([%d.]+)x`)
			if tonumber(tx) + tonumber(tw) > tonumber(cx) + tonumber(cw) then
				overflowed = true
			end
			local beside = math.abs(tonumber(ay) - tonumber(ty)) < tonumber(th)
			-- Beside the tabs, it is whole and inside the card; under the rule,
			-- inside the card at the narrow end where only the tabs run past.
			if beside then
				expect(`{at} whole={tonumber(ax) + tonumber(aw) <= tonumber(cx) + tonumber(cw) - 12}`).to_be(`{at} whole=true`)
			else
				expect(`{at} inside={tonumber(ax) + tonumber(aw) <= tonumber(cx) + tonumber(cw) - 12}`).to_be(`{at} inside=true`)
			end
			if was_beside ~= nil and beside ~= was_beside then
				switches += 1
			end
			was_beside = beside
		end
		-- (Once from under to beside; never back and forth.)
		expect(`{case.tabs} switches={switches}`).to_be(`{case.tabs} switches=1`)
		-- (The tabs did run past the card at the narrow end: the case forced.)
		expect(`{case.tabs} overflowed={overflowed}`).to_be(`{case.tabs} overflowed=true`)
	end
end)

-- Where a catalog's action sits depends only on the width now, never on
-- the way there: swept up and down in steps of 10 in one window, from
-- narrower than Themes' own tabs (they overflow there, whatever the font)
-- to 1100, each width gives the same place both ways.
test("a catalog's action sits in the same place at a width whether widened or narrowed to it", function()
	for _, case in { { fx = "themes_fixture.luau", cmd = "themes", tabs = "Themes", action = "Import" }, { fx = "plugins_fixture.luau", tabs = "Plugins", action = "Install" } } do
		local app = mount(case.fx)
		app:resize(260, 900)
		app:frames(10)
		if case.cmd then
			app:command(case.cmd)
		end
		app:frames(30)
		local function place(width: number): string
			app:resize(width, 900)
			app:frames(20)
			local tree = app:tree()
			local _, ty, _, th = tree:match(`tablist "{case.tabs}" (%-?[%d.]+),(%-?[%d.]+) ([%d.]+)x([%d.]+)`)
			local cx, _, cw = tree:match("#catalog%-card (%-?[%d.]+),(%-?[%d.]+) ([%d.]+)x")
			local ax, ay, aw
			for line in tree:gmatch(`[^\n]*button "{case.action}[^\n]*`) do
				if not line:find("hidden", 1, true) then
					ax, ay, aw = line:match(' (%-?[%d.]+),(%-?[%d.]+) ([%d.]+)x')
				end
			end
			assert(ty and ax and cx, `{case.tabs} {width}: {tree}`)
			-- (Whole and inside its card, wherever it is.)
			expect(`{case.tabs} {width} inside={tonumber(ax) + tonumber(aw) <= tonumber(cx) + tonumber(cw) - 12}`).to_be(`{case.tabs} {width} inside=true`)
			return if math.abs(tonumber(ay) - tonumber(ty)) < tonumber(th) then "beside" else "under"
		end
		local up: { [number]: string } = {}
		for width = 260, 1100, 10 do
			up[width] = place(width)
		end
		for width = 1100, 260, -10 do
			local down = place(width)
			expect(`{case.tabs} {width} {down}`).to_be(`{case.tabs} {width} {up[width]}`)
		end
		-- (Wide enough, it's beside the tabs; narrow, under the rule.)
		expect(`{case.tabs} {up[1100]}`).to_be(`{case.tabs} beside`)
		expect(`{case.tabs} {up[260]}`).to_be(`{case.tabs} under`)
	end
end)

-- Installed's rows short of room give way so the name keeps its least
-- (96) and every control stays whole and inside the card: the badges move
-- under the text first, then Update under its sentence, then the switch
-- and Remove under the text too (wrapping under the badges, in their own
-- order, where even that line is too wide). Where things fit, the row is
-- the row it always was; the place is the width's alone, both ways.
local function focus_row(app: any): { [string]: { x: number, y: number, w: number }? }
	local tree = app:tree("#catalog-card")
	local out: { [string]: { x: number, y: number, w: number }? } = {}
	-- (What's shown: a giving row's copies laid out only to be measured
	-- are hidden.)
	local function box(pattern: string): { x: number, y: number, w: number }?
		for line in tree:gmatch("[^\n]+") do
			local x, y, w = line:match(pattern .. " (%-?[%d.]+),(%-?[%d.]+) ([%d.]+)x")
			if x and not line:find(" hidden", 1, true) then
				return { x = tonumber(x) :: number, y = tonumber(y) :: number, w = tonumber(w) :: number }
			end
		end
		return nil
	end
	local cx, _, cw = tree:match("#catalog%-card (%-?[%d.]+),(%-?[%d.]+) ([%d.]+)x")
	out.card = { x = tonumber(cx) :: number, y = 0, w = tonumber(cw) :: number }
	out.name = box('text "Focus"')
	out.switch = box('switch "Focus"')
	out.remove = box('button "Remove Focus"')
	out.update = box('button "Update"')
	-- (Focus's version: the second row's, after Compact sidebar's.)
	local versions = {}
	for line in tree:gmatch('[^\n]*text "1%.0%.0"[^\n]*') do
		if not line:find("hidden", 1, true) then
			table.insert(versions, line)
		end
	end
	local vx, vy, vw = (versions[2] or ""):match(" (%-?[%d.]+),(%-?[%d.]+) ([%d.]+)x")
	out.version = if vx then { x = tonumber(vx) :: number, y = tonumber(vy) :: number, w = tonumber(vw) :: number } else nil
	return out
end

test("Installed's rows give way short of room: the name keeps its least, every control whole and inside", function()
	-- Where everything fits, as it always was (main's boxes at 1048).
	local wide = mount("plugins_fixture.luau")
	wide:resize(1048, 900)
	wide:frames(30)
	wide:command("update_found")
	wide:frames(30)
	local b = focus_row(wide)
	expect(`{b.version and b.version.x} {b.switch and b.switch.x} {b.remove and b.remove.x} {b.update and b.update.x}`).to_be("678 757 805 603")

	local app = mount("plugins_fixture.luau")
	app:resize(250, 900)
	app:frames(30)
	app:command("update_found")
	app:frames(30)
	local function layout(width: number): string
		app:resize(width, 900)
		app:frames(25)
		local r = focus_row(app)
		local right = r.card.x + r.card.w
		local at = `{width}`
		for _, k in { "switch", "remove", "update", "version" } do
			local c = r[k]
			assert(c, `{at}: no {k}`)
			-- Whole and inside the card.
			expect(`{at} {k} inside={c.x + c.w <= right + 0.5}`).to_be(`{at} {k} inside=true`)
		end
		local name = r.name :: any
		-- Anything on the name's line leaves it its least (where the card
		-- has that much beside the tile at all).
		for _, k in { "switch", "version" } do
			local c = r[k] :: any
			if math.abs(c.y - name.y) < 12 and r.card.w >= 96 + 76 then
				expect(`{at} {k} floor={c.x - name.x >= 96}`).to_be(`{at} {k} floor=true`)
			end
		end
		return `{r.version.x},{r.version.y - name.y}|{r.switch.x},{r.switch.y - name.y}|{r.update.x},{r.update.y - name.y}`
	end
	local up: { [number]: string } = {}
	for width = 250, 1100, 10 do
		up[width] = layout(width)
	end
	for width = 1100, 250, -10 do
		expect(`{width} {layout(width)}`).to_be(`{width} {up[width]}`)
	end
end)

-- A giving row's place is the width's alone, from its first frame: built
-- fresh at a width, it is what the same width reached by resizing up or
-- down gives. Its controls under the text start at the text's start.
test("Installed's rows built fresh at each width are as the same width reached by resizing", function()
	local function layout(app: any, width: number): string
		local r = focus_row(app)
		local name = r.name :: any
		local sw = r.switch :: any
		-- (On a line of its own under the badges: the switch on the text's
		-- start, Focus having no settings of its own.)
		-- (Beside the text it's past the name's least; nearer, it's on its
		-- own line, and that starts on the text's start exactly.)
		local v = r.version :: any
		if sw.x < name.x + 96 and math.abs(sw.y - v.y) > 12 then
			expect(`{width} start={sw.x}`).to_be(`{width} start={name.x}`)
		end
		return `{r.version.x},{r.version.y - name.y}|{sw.x},{sw.y - name.y}|{r.update.x},{r.update.y - name.y}`
	end
	local fresh: { [number]: string } = {}
	for width = 200, 1000, 10 do
		local app = mount("plugins_fixture.luau")
		app:command("update_found")
		app:resize(width, 900)
		app:frames(30)
		fresh[width] = layout(app, width)
	end
	local app = mount("plugins_fixture.luau")
	app:command("update_found")
	app:resize(200, 900)
	app:frames(30)
	for width = 200, 1000, 10 do
		app:resize(width, 900)
		app:frames(25)
		expect(`up {width} {layout(app, width)}`).to_be(`up {width} {fresh[width]}`)
	end
	for width = 1000, 200, -10 do
		app:resize(width, 900)
		app:frames(25)
		expect(`down {width} {layout(app, width)}`).to_be(`down {width} {fresh[width]}`)
	end
end)


test("a giving row crossing a step past 0 moves its parts and builds nothing again; at 0 it builds no line under its text", function()
	local app = mount("plugins_fixture.luau")
	app:command("update_found")
	-- Every node in the card, by its id, shown or out of layout.
	local function ids(): string
		local out = {}
		for id in app:tree("#catalog-card"):gmatch("@(%d+)") do
			table.insert(out, id)
		end
		table.sort(out)
		return table.concat(out, " ")
	end
	-- What's shown of Focus: its name, switch, Remove and Update, each once.
	local function shown(): string
		local n = {}
		for line in app:tree("#catalog-card"):gmatch("[^\n]+") do
			if not line:find(" hidden", 1, true) then
				for _, pattern in { 'text "Focus"', 'switch "Focus"', 'button "Remove Focus"', 'button "Update"' } do
					if line:find(pattern, 1, true) then
						n[pattern] = (n[pattern] or 0) + 1
					end
				end
			end
		end
		return `{n['text "Focus"']} {n['switch "Focus"']} {n['button "Remove Focus"']} {n['button "Update"']}`
	end
	local function hidden(): number
		local n = 0
		for line in app:tree("#catalog-card"):gmatch("[^\n]+") do
			if line:find(" hidden", 1, true) then
				n += 1
			end
		end
		return n
	end
	-- Where everything fits: nothing kept out of layout, the rows as they
	-- always were.
	app:resize(1000, 900)
	app:frames(30)
	expect(hidden()).to_be(0)
	expect(shown()).to_be("1 1 1 1")
	-- Every row past step 0, through the steps past it and back: the same
	-- nodes, each part shown once.
	app:resize(330, 900)
	app:frames(25)
	local wide = ids()
	for _, width in { 300, 260, 230, 200, 260, 330 } do
		app:resize(width, 900)
		app:frames(25)
		expect(`{width} {ids()}`).to_be(`{width} {wide}`)
		expect(`{width} {shown()}`).to_be(`{width} 1 1 1 1`)
	end
end)



test("all under its text, a row's switch begins a line of its own under its badges, on its name's start", function()
	local app = mount("plugins_fixture.luau")
	app:resize(200, 900)
	app:frames(30)
	-- Compact sidebar: its version alone, room for the switch beside it,
	-- and still its switch on a line of its own.
	local tree = app:tree("#catalog-card")
	local function box(pattern: string): { x: number, y: number }?
		for line in tree:gmatch("[^\n]+") do
			local x, y = line:match(pattern .. " (%-?[%d.]+),(%-?[%d.]+) ")
			if x and not line:find(" hidden", 1, true) then
				return { x = tonumber(x) :: number, y = tonumber(y) :: number }
			end
		end
		return nil
	end
	local name = box('text "Compact sidebar"') :: any
	local version = box('text "1%.0%.0"') :: any
	local switch = box('switch "Compact sidebar"') :: any
	expect(`{switch.x}`).to_be(`{name.x}`)
	expect(switch.y > version.y + 12).to_be(true)
end)
