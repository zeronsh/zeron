-- A new Cloud chat's machine size (Work in's Machine row): offered only on
-- Cloud, in the checkout's place, every size with its specs and credit rate as
-- Settings → Cloud lists them; it starts on the account's default, and a pick
-- rides the create (`machine`). A chat already on Cloud names its size on the
-- chip. On ui/chat_fixture.luau, Cloud answered by ui/cloud_stub.luau (sizes
-- Small, Standard, Large, Extra large; the default `default`, Standard); the
-- fixture notes `machine:<id>` for each chat made on Cloud.

local logic = require("../screens/chat/cloud_machine")

local named_test = test
local function test(name: string, body: () -> ())
	named_test(`Machine size: {name}`, body)
end

local MACHINES = {
	{ id = "small", name = "Small", vcpu = 1, memory_gb = 2, disk_gb = 20, rate = 0.5 },
	{ id = "default", name = "Standard", vcpu = 2, memory_gb = 4, disk_gb = 50, rate = 1 },
	{ id = "large", name = "Large", vcpu = 4, memory_gb = 16, disk_gb = 111, rate = 2 },
	{ id = "xlarge", name = "Extra large", vcpu = 8, memory_gb = 32, disk_gb = 400, rate = 5.56 },
}

-- ── What's offered (pure) ─────────────────────────────────────────────────

test("each size is one line: its name, the default said after it, its figures in columns", function()
	local choices = logic.size_choices(MACHINES, "large")
	expect(#choices).to_be(4)
	expect(choices[3].value).to_be("large")
	expect(choices[3].label).to_be("Large")
	expect(choices[3].tag).to_be("Your default")
	expect(choices[3].cells.vcpu).to_be("4")
	expect(choices[3].cells.memory).to_be("16 GB")
	expect(choices[3].cells.disk).to_be("111 GB")
	expect(choices[3].cells.rate).to_be("2×")
	expect(choices[3].bare).to_be(true)
	expect(choices[3].icon).to_be("cpu")
	expect(choices[4].cells.disk).to_be("400 GB")
	expect(choices[4].cells.rate).to_be("5.56×")
	expect(choices[1].tag).to_be(nil)
end)

test("a new chat starts on the pick while it's offered, else the default", function()
	expect(logic.size_for(MACHINES, nil, "default")).to_be("default")
	expect(logic.size_for(MACHINES, "xlarge", "default")).to_be("xlarge")
	expect(logic.size_for(MACHINES, "huge", "large")).to_be("large")
	-- An edge that offers no sizes: none sent, the edge's default.
	expect(logic.size_for({}, "large", "large")).to_be(nil)
	expect(logic.on_size("Cloud", logic.size_name(MACHINES, "large"))).to_be("Cloud · Large")
	expect(logic.on_size("Cloud", logic.size_name(MACHINES, "huge"))).to_be("Cloud")
end)

-- ── On the composer ───────────────────────────────────────────────────────

local function settle(app: any)
	app:frames(30)
end

local function rested(app: any)
	app:wait("#never-shown", { timeout_ms = 900 })
	settle(app)
end

local function showing(app: any, sel: string): boolean
	if not app:has(sel) then
		return false
	end
	local first = string.match(app:tree(sel), "[^\n]+") or ""
	return string.find(first, " hidden", 1, true) == nil
end

local function panel(app: any)
	if not showing(app, 'role:menuitem "Device"') then
		app:click('role:button "Branch"')
		settle(app)
	end
end

local function open_row(app: any, row: string)
	panel(app)
	app:click(`role:menuitem "{row}"`)
	settle(app)
end

local function pick_in(app: any, row: string, option: string)
	open_row(app, row)
	app:click(`role:option ~"{option}"`)
	settle(app)
end

local function close(app: any)
	app:key("Escape")
	app:key("Escape")
	settle(app)
end

local function chosen(app: any, row: string): string
	rested(app)
	local words = {}
	for line in app:tree(`role:menuitem "{row}"`):gmatch("[^\n]+") do
		local t = line:match('text "([^"]*)"')
		if t then
			table.insert(words, t)
		end
	end
	return table.concat(words, " ")
end

local function chip_device(app: any): string
	rested(app)
	return app:text('role:button "Branch" >> #composer-place-device[1]')
end

local function on_cloud(): any
	local app = mount("chat_fixture.luau")
	app:resize(960, 760)
	app:command("go_home")
	settle(app)
	app:command("cloud_stub.offer")
	settle(app)
	pick_in(app, "Device", "Cloud")
	return app
end

local function send(app: any, words: string)
	close(app)
	app:type('role:textbox "Message"', words)
	app:key("Enter")
	settle(app)
end

test("offered on Cloud alone, in the checkout's place", function()
	local app = mount("chat_fixture.luau")
	app:resize(960, 760)
	app:command("go_home")
	settle(app)
	app:command("cloud_stub.offer")
	settle(app)
	panel(app)
	expect(showing(app, 'role:menuitem "Machine"')).to_be(false)
	expect(showing(app, 'role:menuitem "Checkout"')).to_be(true)
	close(app)
	pick_in(app, "Device", "Cloud")
	panel(app)
	expect(showing(app, 'role:menuitem "Machine"')).to_be(true)
	expect(showing(app, 'role:menuitem "Checkout"')).to_be(false)
	expect(app:error()).to_be(nil)
end)

test("starts on the account's default, each size with its specs and rate", function()
	local app = on_cloud()
	expect(chosen(app, "Machine")).to_contain("Standard")
	expect(chip_device(app)).to_be("Cloud · Standard")
	open_row(app, "Machine")
	for _, name in { "Small", "Standard", "Large", "Extra large" } do
		expect(app:has(`role:option ~"{name}"`)).to_be(true)
	end
	expect(app:has('role:option ~"Standard" >> "Your default"')).to_be(true)
	expect(app:has('role:option ~"Large" >> "16 GB"')).to_be(true)
	expect(app:has('role:option ~"Large" >> "2×"')).to_be(true)
	expect(app:has('role:option ~"Large" >> "111 GB"')).to_be(true)
	expect(app:has('role:option ~"Extra large" >> "5.56×"')).to_be(true)
	-- Its columns, named once over the list.
	for _, column in { "vCPU", "Memory", "Disk", "Credits" } do
		expect(app:has(`"{column}"`)).to_be(true)
	end
	-- No tile at their starts: told apart by their words.
	expect(app:has('role:option ~"Large" >> img')).to_be(false)
	-- Sent as it was: the default's id.
	close(app)
	send(app, "Fix the flaky test")
	local calls = app:state("calls")
	expect(calls).to_contain("made:atlas|-|main|-|cloud")
	expect(calls).to_contain("machine:default")
end)

test("a size picked rides the create, then the next chat starts on the default again", function()
	local app = on_cloud()
	pick_in(app, "Machine", "Extra large")
	expect(chosen(app, "Machine")).to_contain("Extra large")
	expect(chip_device(app)).to_be("Cloud · Extra large")
	send(app, "Profile the build")
	expect(app:state("calls")).to_contain("machine:xlarge")
	-- A new chat: the default again.
	app:command("go_home")
	settle(app)
	pick_in(app, "Device", "Cloud")
	expect(chosen(app, "Machine")).to_contain("Standard")
end)

test("follows the account's default as Settings → Cloud changes it", function()
	local app = on_cloud()
	app:command("cloud_stub.default.large")
	rested(app)
	expect(chosen(app, "Machine")).to_contain("Large")
	open_row(app, "Machine")
	expect(app:has('role:option ~"Large" >> "Your default"')).to_be(true)
	expect(app:has('role:option ~"Standard" >> "Your default"')).to_be(false)
end)

test("a chat on Cloud names its machine's size on the chip", function()
	local app = mount("chat_fixture.luau")
	app:resize(960, 760)
	app:command("go_s2")
	settle(app)
	app:command("cloud_stub.offer")
	app:command("cloud_stub.session.ready_large")
	app:command("session_on_cloud")
	app:wait("#never-shown", { timeout_ms = 300 })
	settle(app)
	expect(app:has('~"Cloud · Large"')).to_be(true)
	-- Resized while asleep: the chip follows the session.
	app:command("cloud_stub.session.disk_warn_xlarge")
	app:wait("#never-shown", { timeout_ms = 300 })
	settle(app)
	expect(app:has('~"Cloud · Extra large"')).to_be(true)
	expect(app:error()).to_be(nil)
end)

-- ── One catalogue ─────────────────────────────────────────────────────────
-- The picker and Settings → Cloud read the same sizes and the same default
-- (`cloud.machines()`, `cloud.machine()`: the engine's CloudStatus). With the
-- edge's real catalogue (ui/cloud_stub.luau's REAL_MACHINES, held to
-- edge/src/cloud/policy.ts by src/cloud.rs) and the account's default Large,
-- both say the same of every size.

local REAL = {
	{ name = "Small", vcpu = "2", memory = "4 GB", disk = "51 GB", rate = "0.5×" },
	{ name = "Default", vcpu = "4", memory = "8 GB", disk = "51 GB", rate = "1×" },
	{ name = "Large", vcpu = "8", memory = "16 GB", disk = "111 GB", rate = "2×" },
	{ name = "Extra large", vcpu = "16", memory = "32 GB", disk = "251 GB", rate = "5.56×" },
}

test("the picker and Settings → Cloud show the same catalogue and default", function()
	-- Settings → Cloud.
	local settings = mount("shell_fixture.luau")
	settings:resize(1200, 900)
	settings:frames(30)
	settings:command("settings_stub.sign_in")
	settings:command("cloud_stub.offer")
	settings:command("cloud_stub.real_sizes")
	settings:command("cloud_stub.default.large")
	settings:command("shell.settings")
	settings:frames(20)
	settings:click('#settings-nav >> role:tab "Cloud"')
	rested(settings)
	local group = '#settings-page >> role:radiogroup "Default machine"'
	-- The composer's picker.
	local app = on_cloud()
	app:command("cloud_stub.real_sizes")
	app:command("cloud_stub.default.large")
	rested(app)
	open_row(app, "Machine")
	for _, size in REAL do
		local radio = `{group} >> role:radio "{size.name}"`
		local option = `role:option ~"{size.name}"`
		expect(settings:has(radio)).to_be(true)
		expect(app:has(option)).to_be(true)
		expect(settings:has(`{radio} >> "{size.vcpu} vCPU"`)).to_be(true)
		expect(settings:has(`{radio} >> "{size.memory} memory"`)).to_be(true)
		expect(settings:has(`{radio} >> "{size.rate} credits"`)).to_be(true)
		expect(app:has(`{option} >> "{size.vcpu}"`)).to_be(true)
		expect(app:has(`{option} >> "{size.memory}"`)).to_be(true)
		expect(app:has(`{option} >> "{size.rate}"`)).to_be(true)
		expect(settings:has(`{radio} >> "{size.disk} disk"`)).to_be(true)
		expect(app:has(`{option} >> "{size.disk}"`)).to_be(true)
		-- The default: Settings' Current, the picker's tag.
		local is_default = size.name == "Large"
		expect(settings:has(`{radio} >> ~"Current"`)).to_be(is_default)
		expect(app:has(`{option} >> "Your default"`)).to_be(is_default)
	end
	-- None of the stub's other sizes.
	expect(app:has('role:option ~"Standard"')).to_be(false)
	expect(settings:has(`{group} >> role:radio "Standard"`)).to_be(false)
	-- And a new chat starts on it.
	expect(chosen(app, "Machine")).to_contain("Large")
end)
