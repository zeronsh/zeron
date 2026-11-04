-- What's said for a moment (ds/toast) never dims or holds the window: a
-- picture's Copy Path, in a reply and in the lightbox, says so in the ds
-- stack, never in zgx's own (`ui.toast`), whose layer zgx draws the modal
-- dim behind (the whole window tinted while it shows). In the lightbox the
-- stack shows over the stage, not under its scrim. On
-- ui/reply_pictures_fixture.luau.

local function open(): any
	local app = mount("reply_pictures_fixture.luau")
	-- (Tall enough that the list builds every reply.)
	app:resize(1000, 7000)
	app:frames(30)
	expect(app:error()).to_be_nil()
	return app
end

-- The menu rows drawn now (a closed menu's are listed `hidden`).
local function menu_open(app: any): boolean
	for line in string.gmatch(app:tree(), "[^\n]+") do
		if string.find(line, " menuitem ", 1, true) and not string.find(line, "hidden", 1, true) then
			return true
		end
	end
	return false
end

local function until_(app: any, done: () -> boolean)
	for _ = 1, 40 do
		if done() then
			return
		end
		app:wait("#no-such-node", { timeout_ms = 50 })
	end
end

local function copy_path(app: any, target: string)
	app:right_click(target)
	until_(app, function()
		return menu_open(app)
	end)
	expect(menu_open(app)).to_be(true)
	app:click('role:menuitem "Copy Path"')
	-- (A frame at a time: a success toast goes by itself after a while.)
	for _ = 1, 30 do
		app:frames(1)
		if app:has('~"Path copied"') then
			break
		end
	end
end

-- zgx's toast stack: a layer called Notifications, the window dimmed
-- behind it.
local function zgx_layer(app: any): boolean
	return app:has('role:dialog "Notifications"')
end

test("a reply picture's Copy Path says so in a toast that dims nothing", function()
	local app = open()
	copy_path(app, 'role:button "Open Pixels"')
	expect(app:clipboard()).to_contain("assets/")
	expect(app:has('role:status >> "Path copied"')).to_be(true)
	expect(zgx_layer(app)).to_be(false)
	expect(app:error()).to_be_nil()
end)

test("in the lightbox, Copy Path's toast shows over the stage, the window's own stack standing aside", function()
	local app = open()
	app:click('role:button "Open Pixels"')
	app:frames(60)
	expect(app:has('"3 of 3"')).to_be(true)
	copy_path(app, 'role:dialog "Image viewer"')
	expect(app:has('role:dialog "Image viewer" >> role:status >> "Path copied"')).to_be(true)
	-- (Once: not again under the scrim.)
	expect(app:count('role:status >> "Path copied"')).to_be(1)
	expect(zgx_layer(app)).to_be(false)
	-- Closed, it's the window's stack that shows it.
	app:key("Escape")
	app:frames(60)
	expect(app:state("ds/lightbox.luau::open")).to_be_nil()
	expect(app:count('role:status >> "Path copied"')).to_be(1)
	expect(app:has('role:dialog "Image viewer" >> role:status >> "Path copied"')).to_be(false)
	expect(app:error()).to_be_nil()
end)
