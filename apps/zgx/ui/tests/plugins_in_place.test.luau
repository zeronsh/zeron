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

test("one Zeron can't vouch for says Unverified by its version, and where it came from", function()
	local app = open()
	local function badges(): number
		local _, n = string.gsub(app:tree(), '"Unverified"', "")
		return n
	end
	-- Zeron's own, yours and a verified author's: never.
	app:command("builtin")
	app:frames(20)
	expect(badges()).to_be(0)
	-- Someone Zeron can't vouch for: one badge, theirs.
	app:command("arrive")
	app:frames(20)
	expect(badges()).to_be(1)
	expect(app:tree()).to_contain("Unknown author")
	-- Where it came from, once known, in place of "Unknown author".
	app:command("ping_from_github")
	app:frames(20)
	expect(app:tree()).to_contain("From github.com/eve/ping")
	expect(app:tree()).never.to_contain("Unknown author")
	expect(badges()).to_be(1)
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
