-- An orchestrator's settings (screens/orchestrator/dialogs' sheet) say its
-- crew as the engine reads it (crates/proto CrewPolicy): no limit reads
-- "No limit", never 0; and with no device chosen, its crew runs on its own
-- device, so that one shows ticked, never "runs nowhere". Saved untouched,
-- nothing about its crew is sent. On ui/orchestrator_fixture.luau, where
-- Exnomic VPN (orc-vpn, on desk) has no limit and its crew on desk.

local function settle(app: any)
	app:frames(30)
end

local function open(id: string): any
	local app = mount("orchestrator_fixture.luau")
	app:resize(1440, 900)
	settle(app)
	app:set_state("orch_settings_of", id)
	app:set_state("orch_settings_open", true)
	settle(app)
	return app
end

local function line_of(app: any, sel: string): string
	return string.match(app:tree(sel), "[^\n]+") or ""
end

local function checked(app: any, label: string): boolean
	local line = line_of(app, `#orch-settings >> role:checkbox "{label}"`)
	return string.find(line, " checked$") ~= nil or string.find(line, " checked ", 1, true) ~= nil
end

test("settings crew: no limit reads No limit, and saves as none", function()
	local app = open("orc-vpn")
	expect(app:state("orch_set_limit")).to_be(0)
	local stepper = app:tree('#orch-settings >> role:spinbutton "At once"') :: string
	expect(stepper).to_contain("No limit")
	expect(string.find(stepper, '="0"', 1, true)).to_be(nil)
	-- A limit reads as its number.
	app:set_state("orch_set_limit", 4)
	settle(app)
	expect(app:tree('#orch-settings >> role:spinbutton "At once"')).to_contain("4")
end)

test("settings crew: none chosen shows its own device ticked", function()
	local app = open("orc-vpn")
	-- (Nothing chosen: what a new orchestrator has.)
	app:set_state("orch_set_devices", {})
	settle(app)
	expect(checked(app, "desk")).to_be(true)
	expect(checked(app, "macbook")).to_be(false)
	-- Saved as it is, its crew isn't sent: it runs where it did.
	app:click("#orch-settings-save")
	settle(app)
	expect(string.find(app:state("orch_calls") :: string, "crew=", 1, true)).to_be(nil)
end)

test("settings crew: ticking another device keeps its own", function()
	local app = open("orc-vpn")
	app:set_state("orch_set_devices", {})
	settle(app)
	app:click('#orch-settings >> role:checkbox "macbook"')
	settle(app)
	expect(checked(app, "desk")).to_be(true)
	expect(checked(app, "macbook")).to_be(true)
	local d = app:state("orch_set_devices") :: { [string]: boolean }
	expect(d["dev-desk"] == true and d["dev-mac"] == true).to_be(true)
	app:click("#orch-settings-save")
	settle(app)
	expect(app:state("orch_calls")).to_contain("crew={…}")
end)
