-- A launch gives the composer the keys: what's typed as the window opens
-- goes into it, with no click first, whether the window opens on a new chat
-- or on the session it was left on. (ademark's A1 types from the launch and
-- reads the composer; with the keys nowhere, every run timed out.) On the
-- shell in ui/shell_fixture.luau; a second `mount` is the next launch.

local BOX = 'role:textbox "Message"'
local SIDE = "#shell-sidebar"

local function focused(app: any): string
	for line in app:tree():gmatch("[^\n]+") do
		if line:find(" focused", 1, true) then
			return line
		end
	end
	return ""
end

local function has_keys(app: any): boolean
	return focused(app):find('textbox "Message"', 1, true) ~= nil
end

local function launch(): any
	local app = mount("shell_fixture.luau")
	app:resize(1600, 1000)
	return app
end

test("a launch on a new chat: the composer has the keys from the first frame", function()
	local app = launch()
	app:frames(1)
	expect(has_keys(app)).to_be(true)
	app:frames(30)
	expect(has_keys(app)).to_be(true)
end)

test("a launch on the session it was left on: its composer has the keys", function()
	local first = launch()
	first:frames(30)
	first:click(`{SIDE} >> role:button "Fix the build"`)
	first:frames(30)
	local app = launch()
	app:frames(1)
	expect(app:tree()).to_contain("Fix the build")
	expect(has_keys(app)).to_be(true)
	app:frames(30)
	expect(has_keys(app)).to_be(true)
end)
