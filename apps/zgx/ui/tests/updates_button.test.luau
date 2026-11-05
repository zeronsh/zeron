-- The top bar's update button (screens/shell/updates): only with something to
-- say, three marks at most, and its list by device with what each row can do.

local function row(device: string, name: string, harness: string, action: string?, connected: boolean?): any
	return {
		id = `{device}:{harness}`,
		device = device,
		device_name = device,
		here = device == "here",
		connected = connected ~= false,
		kind = "agent",
		harness = harness,
		name = name,
		mark = "claude",
		phase = "available",
		installed = "1.0",
		latest = "1.1",
		action = action,
		active = false,
	}
end

local function open(rows: { any }): any
	local app = mount("updates_fixture.luau")
	app:resize(900, 700)
	app:set_state("updates", { rows = rows })
	app:frames(10)
	return app
end

test("nothing to update shows nothing", function()
	local app = open({})
	expect(app:has("#updates-button")).to_be(false)
end)

test("the button wears three marks at most and says how many", function()
	local app = open({
		row("here", "Claude Code", "claude-code", "update"),
		row("here", "Codex", "codex", "update"),
		row("metal", "Claude Code", "claude-code", "update"),
		row("metal", "Grok", "grok", "update"),
		row("metal", "Pi", "pi", "update"),
	})
	expect(app:has('role:button "5 updates"')).to_be(true)
	expect(app:has("#updates-mark-3")).to_be(true)
	expect(app:has("#updates-mark-4")).to_be(false)
end)

test("the same thing on two devices is one mark", function()
	local app = open({
		row("here", "Claude Code", "claude-code", "update"),
		row("metal", "Claude Code", "claude-code", "update"),
	})
	expect(app:has("#updates-mark-1")).to_be(true)
	expect(app:has("#updates-mark-2")).to_be(false)
end)

test("the list goes by device, and offers what each row can do", function()
	local app = open({
		row("here", "Claude Code", "claude-code", "update"),
		row("metal", "Codex", "codex", "steps"),
		row("metal", "Grok", "grok", "update", false),
	})
	app:click("#updates-button")
	app:frames(10)
	expect(app:has('"metal"')).to_be(true)
	expect(app:has('role:button "Update · Claude Code"')).to_be(true)
	expect(app:has('role:button "View steps · Codex"')).to_be(true)
	-- A device out of reach says so, and its action waits.
	expect(app:has('"Offline"')).to_be(true)
end)

test("another device's Zeron says when it updates there, and what stopping costs", function()
	local app = open({
		{
			id = "mac:zeron",
			device = "mac",
			device_name = "mac",
			here = false,
			connected = true,
			kind = "zeron",
			phase = "available",
			installed = "1.0",
			latest = "1.1",
			active = false,
		},
		{
			id = "metal:zeron",
			device = "metal",
			device_name = "metal",
			here = false,
			connected = true,
			kind = "zeron",
			phase = "available",
			installed = "1.0",
			latest = "1.1",
			action = "update",
			active = false,
		},
	})
	app:click("#updates-button")
	app:frames(10)
	expect(app:has('"1.0 → 1.1 · update from that device"')).to_be(true)
	expect(app:has('role:button "Update · Zeron"')).to_be(true)
end)

test("Update all says what it will do first, and what it can't reach", function()
	local app = open({
		row("here", "Claude Code", "claude-code", "update"),
		row("metal", "Grok", "grok", "update"),
		row("metal", "Codex", "codex", "steps"),
		row("away", "Pi", "pi", "update", false),
	})
	app:click("#updates-button")
	app:frames(10)
	app:click("#updates-all")
	app:frames(10)
	expect(app:has("#updates-confirm")).to_be(true)
	expect(app:has('"Updates 2 agents. Each waits for what\'s running to finish"')).to_be(true)
	expect(app:has('"Skips Codex on metal: needs doing by hand"')).to_be(true)
	expect(app:has('"Skips Pi on away: Offline"')).to_be(true)
	-- Nothing happens until it's said yes to.
	app:click("#updates-confirm-cancel")
	app:frames(10)
	expect(app:has("#updates-confirm")).to_be(false)
end)

test("a device that says it can't update from here says why, and offers what to run", function()
	local app = open({
		{
			id = "box:zeron",
			device = "box",
			device_name = "box",
			here = false,
			connected = true,
			kind = "zeron",
			phase = "available",
			installed = "1.0",
			latest = "1.1",
			why_not = "It was started by hand",
			manual_command = "zeron update",
			action = "copy",
			active = false,
		},
	})
	app:click("#updates-button")
	app:frames(10)
	expect(app:has('"It was started by hand"')).to_be(true)
	expect(app:has('role:button "Copy command · Zeron"')).to_be(true)
end)
