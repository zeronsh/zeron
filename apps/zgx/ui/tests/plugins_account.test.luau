-- Settings → Plugins with a plugin whose switch is the account's
-- (`"switch": { "scope": "account" }`, Zeron's own alone): it says so on
-- its row, on every computer's list, and its switch names no device, set
-- here from any of them; a plugin of a device's own still names it. On the
-- page in ui/plugins_account_fixture.luau.

local function open(): any
	local app = mount("plugins_account_fixture.luau")
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

test("plugins of a device's own carry no account-wide tag", function()
	local app = open()
	expect(app:error()).to_be_nil()
	expect(app:tree()).never.to_contain("All devices")
	pick(app, "devbox")
	expect(app:tree()).never.to_contain("All devices")
end)

test("one whose switch is the account's has one switch, set here from any computer", function()
	local app = open()
	app:command("with_account")
	app:frames(20)
	expect(app:tree()).to_contain("Orchestrators")
	expect(app:tree()).to_contain("All devices")
	app:click('role:switch "Orchestrators"')
	landed(app)
	expect(asked(app, "set_off:orchestrators:false@")).to_be(true)
	-- Another computer's list shows it the same, and its switch names no
	-- device: it's the account's, set here.
	pick(app, "devbox")
	expect(app:tree()).to_contain("All devices")
	expect(app:tree()).to_contain("Checkout status")
	app:click('role:switch "Orchestrators"')
	landed(app)
	expect(asked(app, "set_off:orchestrators:true@")).to_be(true)
	expect(calls(app)).never.to_contain("set_off:orchestrators:true@devbox")
	expect(calls(app)).never.to_contain("set_off:orchestrators:false@devbox")
	-- Its own plugins still name it.
	app:click('role:switch "Checkout status"')
	landed(app)
	expect(calls(app)).to_contain("set_off:checkout:true@devbox")
	expect(app:error()).to_be_nil()
end)
