-- Settings → Providers' Usage sheet: one button on the page opens every
-- provider's limits together, pooled across agents; Refresh asks the same
-- device account cache composers use for a fresh probe.

local PAGE = "#settings-page"
local NAV = "#settings-nav"

local function open(): any
	local app = mount("shell_fixture.luau")
	app:resize(1200, 900)
	app:frames(30)
	app:command("settings_stub.sign_in")
	app:command("shell.settings")
	app:frames(20)
	app:click(`{NAV} >> role:tab "Providers"`)
	app:frames(30)
	return app
end

local function accounts(): any
	return {
		codex = {
			{
				id = "c1",
				email = "ada@example.com",
				plan = "Pro",
				active = true,
				can_switch = true,
				windows = {
					{ label = "Session", used = 0.25, resets_at = os.time() + 7200 },
					{ label = "Week", used = 0.5, resets_at = os.time() + 3 * 86400 },
				},
			},
		},
		pi = {
			{
				id = "p1",
				email = "ada@example.com",
				plan = "Pro",
				provider = "openai-codex",
				active = true,
				can_switch = true,
				windows = { { label = "Week", used = 0.5 } },
			},
		},
		["claude-code"] = {
			{
				id = "a1",
				email = "claude@example.com",
				plan = "Max",
				active = true,
				can_switch = true,
				windows = { { label = "Session", used = 0.8 }, { label = "Opus", used = 0.95 } },
			},
		},
	}
end

test("usage sheet opens every provider's limits together, deduplicated across agents", function()
	local app = open()
	app:set_state("agents.on_device", { [""] = accounts() })
	app:click(`{PAGE} >> role:button "Usage"`)
	app:frames(40)
	expect(app:has('role:dialog "Usage"')).to_be(true)
	local tree = app:tree('role:dialog "Usage"')
	expect(tree).to_contain("ChatGPT")
	expect(tree).to_contain("Claude")
	expect(tree).to_contain("ada@example.com")
	expect(tree).to_contain("claude@example.com")
	expect(tree).to_contain("Session")
	expect(tree).to_contain("Opus")
	expect(tree).to_contain("95%")
	-- The ChatGPT sign-in is one account even though Codex and Pi both know it.
	expect(tree).to_contain("1 account")
	app:click('role:dialog "Usage" >> role:button "Refresh"')
	app:frames(10)
	expect(app:state("settings_stub_calls")).to_contain("agents.refresh_device:")
end)

test("usage sheet tokens tab totals tokens by time agent and model", function()
	local app = open()
	app:set_state("agents.on_device", { [""] = accounts() })
	app:set_state("settings_stub_usage_summary", {
		totals = {
			events = 3,
			inputTokens = 900000,
			outputTokens = 330000,
			totalTokens = 1230000,
			costUsd = 0.0042,
			costEvents = 1,
		},
		buckets = {
			{ start = os.time() - 86400, inputTokens = 500000, outputTokens = 200000, totalTokens = 700000 },
			{ start = os.time(), inputTokens = 400000, outputTokens = 130000, totalTokens = 530000 },
		},
		byHarness = { { harness = "claude-code", totalTokens = 1230000, costUsd = 0.0042 } },
		byModel = { { model = "claude-sonnet-4.5", totalTokens = 1230000 } },
		byAccount = {},
		coverage = { startedCollectingAt = os.time() - 86400 },
	})
	app:click(`{PAGE} >> role:button "Usage"`)
	app:frames(20)
	app:click('role:dialog "Usage" >> role:tab "Tokens"')
	app:frames(40)
	local tree = app:tree('role:dialog "Usage"')
	expect(tree).to_contain("Total tokens")
	expect(tree).to_contain("1.2M")
	expect(tree).to_contain("900.0k")
	expect(tree).to_contain("$0.0042")
	expect(tree).to_contain("claude-code")
	expect(tree).to_contain("claude-sonnet-4.5")
	expect(tree).to_contain("Measured on this machine")
	expect(app:state("settings_stub_calls")).to_contain("agents.usage_summary:")
end)
