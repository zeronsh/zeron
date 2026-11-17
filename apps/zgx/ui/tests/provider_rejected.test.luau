-- A saved login whose key its provider rejected (Cursor's, revoked by a
-- re-login): its row says so in place of its plan, has Remove at hand, and
-- never offers to switch to it — not on the row, not behind its ⋯. On
-- ui/provider_rejected_fixture.luau.

local function open(): any
	local app = mount("provider_rejected_fixture.luau")
	app:resize(900, 1200)
	app:frames(30)
	return app
end

local function shows(app: any, text: string): boolean
	return string.find(app:tree(), text, 1, true) ~= nil
end

-- The node of `kind "label"` that first follows `email`'s line: that
-- account's own (`@id`), as the tree lists a row's nodes after its address.
local function after(app: any, email: string, kind: string, label: string): string?
	local seen = false
	for line in app:tree():gmatch("[^\n]+") do
		if line:find(`text "{email}"`, 1, true) then
			seen = true
		elseif line:find(`text "`, 1, true) and line:find("@example.com", 1, true) then
			seen = false
		elseif seen and line:find(` {kind} "{label}"`, 1, true) then
			return line:match("(@%d+)")
		end
	end
	return nil
end

-- Whether a menu row shows (open: not hidden as it leaves).
local function menu_shows(app: any, label: string): boolean
	for line in app:tree():gmatch("[^\n]+") do
		if line:find(`"{label}"`, 1, true) and not line:find(" hidden", 1, true) then
			return true
		end
	end
	return false
end

test("a rejected key's row says so and offers Remove, not a switch", function()
	local app = open()
	expect(shows(app, "Key rejected — sign in again")).to_be(true)
	-- (In place of its plan line.)
	expect(shows(app, "Key expires Feb 1")).to_be(false)
	local remove = after(app, "ada@example.com", "button", "Remove")
	expect(remove ~= nil).to_be(true)
	-- A working account has no such button: its meters instead.
	expect(after(app, "cy@example.com", "button", "Remove")).to_be(nil)
	expect(after(app, "cy@example.com", "status", "Session usage, 10%") ~= nil).to_be(true)

	app:click(remove :: string)
	app:frames(5)
	expect(app:state("calls")).to_be("remove:dead")
end)

test("a rejected key's actions have no switch; a working one's do", function()
	local app = open()
	app:click(after(app, "ada@example.com", "button", "Account actions") :: string)
	app:frames(10)
	expect(menu_shows(app, "Remove account")).to_be(true)
	expect(menu_shows(app, "Switch to this account")).to_be(false)
	app:key("Escape")
	app:frames(30)

	app:click(after(app, "cy@example.com", "button", "Account actions") :: string)
	app:frames(10)
	expect(menu_shows(app, "Switch to this account")).to_be(true)
end)
