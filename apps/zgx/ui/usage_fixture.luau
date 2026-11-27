-- The Usage sheet open on its Tokens tab over the shell, with a day of
-- made-up usage across three agents: for pictures of the sheet
-- (`fixture=usage_fixture.luau --inspect`, then `zgx inspect snapshot`).
-- `usage_fixture.range` picks 24h, 7d or 30d; `usage_fixture.tab` the tab.

local root = require("./shell_fixture")

local HOUR = 3600
local now = os.time()
local this_hour = (now // HOUR) * HOUR

-- Hour offsets (back from now) and what each agent spent then.
local SHAPE: { { number, number, number, number } } = {
	{ 10, 271_300, 0, 0 },
	{ 9, 863_900, 0, 0 },
	{ 8, 214_700, 0, 0 },
	{ 7, 284_700, 0, 0 },
	{ 6, 2_900_000, 400_000, 0 },
	{ 5, 1_400_000, 0, 600_000 },
	{ 4, 13_100_000, 486_500, 4_000_000 },
	{ 3, 3_200_000, 0, 1_500_000 },
	{ 0, 6_500_000, 0, 0 },
}

local buckets = {}
local totals = { claude = 0, codex = 0, devin = 0 }
for _, row in SHAPE do
	local claude, codex, devin = row[2], row[3], row[4]
	local sum = claude + codex + devin
	totals.claude += claude
	totals.codex += codex
	totals.devin += devin
	local parts = {}
	if claude > 0 then
		table.insert(parts, { harness = "claude-code", totalTokens = claude })
	end
	if codex > 0 then
		table.insert(parts, { harness = "codex", totalTokens = codex })
	end
	if devin > 0 then
		table.insert(parts, { harness = "devin", totalTokens = devin })
	end
	table.insert(buckets, {
		start = this_hour - row[1] * HOUR,
		inputTokens = sum * 0.19,
		outputTokens = sum * 0.02,
		totalTokens = sum,
		byHarness = parts,
	})
end
local all = totals.claude + totals.codex + totals.devin

ui.state("settings_stub_usage_summary", {} :: any):set({
	totals = {
		events = 1834,
		inputTokens = 6_900_000,
		outputTokens = 698_600,
		cacheReadInputTokens = all - 6_900_000 - 698_600 - 1_200_000,
		cacheCreationInputTokens = 1_200_000,
		reasoningTokens = 212_000,
		totalTokens = all,
		costUsd = 108.4086,
		costEvents = 1500,
	},
	bucketSeconds = HOUR,
	buckets = buckets,
	byHarness = {
		{ harness = "claude-code", totalTokens = totals.claude, costUsd = 108.4086 },
		{ harness = "codex", totalTokens = totals.codex },
		{ harness = "devin", totalTokens = totals.devin },
	},
	byModel = {
		{ model = "claude-fable-5-2", totalTokens = 8_200_000, costUsd = 31.2 },
		{ model = "claude-opus-5-5", totalTokens = 20_600_000, costUsd = 77.2086 },
		{ model = "gpt-6.2-sol", totalTokens = 886_500 },
		{ model = "swe-2-1-medium", totalTokens = 6_100_000 },
	},
	byAccount = {},
	coverage = { startedCollectingAt = now - 3 * HOUR, backfillTimestampsEstimated = true },
})

ui.state("settings_usage_range", "7d"):set(ui.state("usage_fixture.range", "24h"):peek())
ui.state("settings_usage_tab", "limits"):set(ui.state("usage_fixture.tab", "tokens"):peek())
require("@app/screens/settings/usage_open"):set(true)

return root
