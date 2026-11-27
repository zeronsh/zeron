-- The Usage sheet's pooling (account/usage_pools): one upstream sign-in
-- appearing under several agents becomes one account, every provider window
-- stays intact, reset times and pace survive, and quotas with different
-- scopes never fold into each other.

local pools = require("../account/usage_pools")

local NOW = 1_800_000_000

test("usage pools fold one sign-in under several agents into one account", function()
	local row = {
		id = "a1",
		email = "ada@example.com",
		plan = "Pro",
		active = true,
		windows = {
			{ label = "Session", used = 0.2 },
			{ label = "Week", used = 0.4 },
		},
	}
	local pi = table.clone(row)
	pi.provider = "openai-codex"
	local opencode = table.clone(row)
	opencode.provider = "openai-codex"
	local out = pools.collect({
		codex = { row },
		pi = { pi },
		opencode = { opencode },
	}, NOW)
	expect(out.total_accounts).to_be(1)
	expect(#out.pools).to_be(1)
	expect(out.pools[1].id).to_be("chatgpt")
	local account = out.pools[1].accounts[1]
	expect(#account.harnesses).to_be(3)
	expect(#account.windows).to_be(2)
	expect(out.total_windows).to_be(2)
end)

test("usage pools keep window labels, resets and pace", function()
	local reset = NOW + 3600 -- one hour left of a five-hour session: 80% elapsed
	local out = pools.collect({
		["claude-code"] = {
			{
				id = "c1",
				email = "claude@example.com",
				plan = "Max",
				active = true,
				windows = {
					{ label = "Session", used = 0.4, resets_at = reset },
					{ label = "Opus", used = 0.9 },
				},
			},
		},
	}, NOW)
	local account = out.pools[1].accounts[1]
	expect(account.windows[1].label).to_be("Session")
	expect(account.windows[1].resets_at).to_be(reset)
	expect(account.windows[1].pace).to_be(0.5)
	expect(account.windows[2].label).to_be("Opus")
	expect(account.windows[2].pace).to_be_nil()
end)

test("usage pools keep scoped quota labels separate", function()
	local out = pools.collect({
		antigravity = {
			{
				id = "g1",
				email = "google@example.com",
				plan = "Paid",
				active = true,
				windows = {
					{ label = "Gemini Session", used = 0.1 },
					{ label = "Claude & GPT Session", used = 0.2 },
				},
			},
		},
	}, NOW)
	expect(#out.pools[1].accounts[1].windows).to_be(2)
end)

test("usage pools keep unknown signed-in rows separate", function()
	local row = { id = "one", email = "Signed in", plan = "", active = false, windows = {} }
	local other = table.clone(row)
	other.id = "two"
	local out = pools.collect({ fx = { row, other } }, NOW)
	expect(out.total_accounts).to_be(2)
end)
