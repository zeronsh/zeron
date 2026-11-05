-- Settings → Plugins on more than one computer: Providers' device choice
-- at the title's end shows another device's plugins, and everything done
-- there names that device (switching, reviewing, removing, installing,
-- updating, checking sources); a device that's away says so in plain
-- words; and a file installs only on this Mac. On the page in
-- ui/plugins_devices_fixture.luau.

local function open(): any
	local app = mount("plugins_devices_fixture.luau")
	app:resize(1000, 900)
	app:frames(20)
	return app
end

local function calls(app: any): string
	return (app:state("calls") :: any) or ""
end

-- Whether the host was asked exactly `entry`.
local function asked(app: any, entry: string): boolean
	for part in string.gmatch(calls(app), "[^,]+") do
		if part == entry then
			return true
		end
	end
	return false
end

local function pick(app: any, name: string)
	app:click('role:button "Device"')
	app:frames(10)
	app:click(`role:menuitem "{name}"`)
	app:frames(20)
end

-- (Time passes in a wait, not frames: a switch asks once its knob lands.)
local function landed(app: any)
	app:wait("#the-knob-lands", { timeout_ms = 400 })
	app:frames(10)
end

test("choosing another computer shows its plugins, and this Mac's again", function()
	local app = open()
	expect(app:tree()).to_contain("Focus")
	expect(asked(app, "check:")).to_be(true)
	pick(app, "devbox")
	expect(calls(app)).to_contain("target:devbox")
	local tree = app:tree()
	expect(tree).to_contain("Checkout status")
	expect(tree).to_contain("Ping")
	expect(tree).never.to_contain("Compact sidebar")
	-- Its sources are checked as it's shown.
	expect(calls(app)).to_contain("check:devbox")
	-- Its icon came with its listing: drawn, not its initial's tile (as
	-- Ping, which has none, shows).
	expect(app:tree()).never.to_contain(' text "C" ')
	expect(app:tree()).to_contain(' text "P" ')
	pick(app, "MacBook Pro")
	expect(asked(app, "target:")).to_be(true)
	expect(app:tree()).to_contain("Compact sidebar")
	expect(app:tree()).never.to_contain("Checkout status")
end)

test("every action on another computer names it", function()
	local app = open()
	pick(app, "devbox")
	app:click('role:switch "Checkout status"')
	landed(app)
	expect(calls(app)).to_contain("set_off:checkout:true@devbox")

	app:click('role:button "Review"')
	app:frames(10)
	expect(calls(app)).to_contain("review:ping@devbox:devbox")

	app:click('role:button "Update"')
	app:frames(10)
	expect(calls(app)).to_contain("update:checkout@devbox")

	app:click('role:button "Remove Ping"')
	app:frames(20)
	expect(app:tree()).to_contain("on devbox")
	app:click('role:button "Remove"')
	app:frames(20)
	expect(calls(app)).to_contain("remove:ping@devbox")
end)

test("a dialog opened on one computer acts there, even after a switch", function()
	local app = open()
	pick(app, "devbox")
	app:click('role:button "Remove Ping"')
	app:frames(20)
	-- (This Mac's list shown underneath before it's confirmed.)
	app:command("switch_here")
	app:frames(5)
	app:click('role:button "Remove"')
	app:frames(20)
	expect(calls(app)).to_contain("remove:ping@devbox")
end)

test("this Mac's actions name no device; another computer's folders aren't offered", function()
	local app = open()
	app:click('role:switch "Focus"')
	landed(app)
	expect(asked(app, "set_off:focus:true@")).to_be(true)
	pick(app, "devbox")
	-- One that can't load there says why, with no folder to show here.
	expect(app:tree()).to_contain("permission Zeron doesn")
	expect(app:has('role:button "Show folder"')).to_be(false)
	-- None there: a link to install one, not this Mac's folder.
	app:command("devbox_empty")
	app:frames(30)
	expect(app:has('role:button "Show plugins folder"')).to_be(false)
	expect(app:has('role:button "Install from a link"')).to_be(true)
	app:click('role:button "Install from a link"')
	app:frames(20)
	expect(app:tree()).to_contain("Install a plugin on devbox")
end)

test("a computer that's away says so, and is looked at again when it's back", function()
	local app = open()
	pick(app, "Old laptop")
	local tree = app:tree()
	expect(tree).to_contain("Old laptop is offline")
	-- (Its counts aren't known: never a 0 that says it has none.)
	expect(tree).to_contain('"–"')
	expect(tree).never.to_contain(' text "0" ')
	expect(tree).to_contain("once it's online again")
	expect(tree).never.to_contain("Focus")

	-- The devbox, listed, stops answering: the same words, and Try again.
	pick(app, "devbox")
	expect(app:tree()).to_contain("Checkout status")
	app:command("unreachable")
	app:frames(20)
	expect(app:tree()).to_contain("devbox is offline")
	app:command("reachable")
	app:click('role:button "Try again"')
	app:frames(20)
	expect(app:tree()).to_contain("Checkout status")

	-- Its presence going and coming back looks again by itself.
	app:command("unreachable")
	app:frames(5)
	app:command("devbox_away")
	app:frames(10)
	expect(app:tree()).to_contain("devbox is offline")
	app:command("reachable")
	app:command("devbox_away")
	app:frames(20)
	expect(app:tree()).to_contain("Checkout status")
end)

test("a file installs only on this Mac; a link installs on any computer", function()
	local app = open()
	app:click('role:button "Install"')
	app:frames(20)
	expect(app:has('role:button "Choose File"')).to_be(true)
	app:click('role:button "Cancel"')
	app:frames(20)

	pick(app, "devbox")
	app:click('role:button "Install"')
	app:frames(20)
	expect(app:tree()).to_contain("Install a plugin on devbox")
	expect(app:has('role:button "Choose File"')).to_be(false)
	expect(app:tree()).to_contain("installs only on the computer it")
	app:type('role:textbox "Link"', "ada/clock")
	app:click('role:dialog >> role:button "Install"')
	app:frames(20)
	expect(calls(app)).to_contain("install:ada/clock@devbox")
end)

-- A node's box from its tree line: x, y, w, h.
local function box(app: any, target: string): (number, number, number, number)
	local line = string.match(app:tree(target), "[^\n]+") or ""
	local x, y, w, h = line:match(" (%-?[%d.]+),(%-?[%d.]+) ([%d.]+)x([%d.]+)")
	return tonumber(x) or 0, tonumber(y) or 0, tonumber(w) or 0, tonumber(h) or 0
end

test("the device choice sits at the top right, centred on the title's line; short of room, under it", function()
	local app = open()
	local _, ty, _, th = box(app, 'role:heading "Plugins"')
	local px, py, pw, ph = box(app, 'role:button "Device"')
	expect(th > 0 and ph > 0).to_be(true)
	expect(math.abs((py + ph / 2) - (ty + th / 2)) <= 1).to_be(true)
	-- At the far end of the title's row: past the description's measure.
	local dx, _, dw = box(app, '"What plugins add to Zeron on each of your computers, and the tools they give your agents."')
	expect(px + pw > dx + dw or dw == 0).to_be(true)
	-- Narrow: under the title, as every page's trail goes.
	app:resize(420, 900)
	app:frames(20)
	local _, ty2, _, th2 = box(app, 'role:heading "Plugins"')
	local _, py2 = box(app, 'role:button "Device"')
	expect(py2 >= ty2 + th2).to_be(true)
end)
