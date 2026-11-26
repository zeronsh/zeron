-- Plan-limit accounts pooled by the service they draw on, for the Usage
-- sheet. The host lists accounts by agent; the same sign-in often appears
-- under several agents (one Claude login under Claude Code and Pi, one
-- ChatGPT login under Codex, Pi, OpenCode and Hermes). This module folds
-- those rows back into one account and keeps every reported window intact:
-- labels, reset times and probe errors are data, never reduced away here.
-- It is pure Lua so its rules are tested without mounting a screen.

local M = {}

export type Window = {
	label: string,
	used: number,
	resets_at: number?,
	kind: string?,
	window_seconds: number?,
	pace: number?,
}
export type Account = {
	key: string,
	email: string,
	plan: string,
	active: boolean,
	key_account: boolean,
	error: string?,
	usage_fetched_at: number?,
	harnesses: { string },
	windows: { Window },
	worst: number,
}
export type Pool = {
	id: string,
	title: string,
	accounts: { Account },
	worst: number,
	unreported: number,
}

local ORDER = {
	anthropic = 1,
	chatgpt = 2,
	copilot = 3,
	google = 4,
	cursor = 5,
	xai = 6,
	opencode = 7,
	nous = 8,
	devin = 9,
	meta = 10,
	fx = 11,
	other = 99,
}
local TITLES = {
	anthropic = "Claude",
	chatgpt = "ChatGPT",
	copilot = "GitHub Copilot",
	google = "Google / Antigravity",
	cursor = "Cursor",
	xai = "xAI / Grok",
	opencode = "OpenCode",
	nous = "Nous",
	devin = "Devin",
	meta = "Meta / Muse",
	fx = "fx",
	other = "Other providers",
}
local HARNESS_FAMILY = {
	["claude-code"] = "anthropic",
	codex = "chatgpt",
	cursor = "cursor",
	devin = "devin",
	grok = "xai",
	antigravity = "google",
	muse = "meta",
	fx = "fx",
}
local PROVIDER_FAMILY = {
	anthropic = "anthropic",
	claude = "anthropic",
	["openai-codex"] = "chatgpt",
	openai = "chatgpt",
	["openai-api"] = "chatgpt",
	copilot = "copilot",
	["github-copilot"] = "copilot",
	google = "google",
	gemini = "google",
	antigravity = "google",
	xai = "xai",
	["xai-oauth"] = "xai",
	grok = "xai",
	opencode = "opencode",
	["opencode-go"] = "opencode",
	nous = "nous",
	devin = "devin",
	meta = "meta",
}

local function lower(s: string?): string
	return string.lower(s or "")
end

local function family(harness: string, provider: string?): string
	if provider and PROVIDER_FAMILY[provider] then
		return PROVIDER_FAMILY[provider]
	end
	return HARNESS_FAMILY[harness] or "other"
end

-- The duration a label implies, where the provider's name for the window
-- says it. Pace is only meaningful against the window's own length; a
-- window whose length is unknown carries none.
local function window_shape(label: string): (string?, number?)
	local l = lower(label)
	if string.find(l, "week", 1, true) or string.find(l, "7d", 1, true) or string.find(l, "7 d", 1, true) then
		return "week", 7 * 86400
	end
	if string.find(l, "session", 1, true) or string.find(l, "5h", 1, true) or string.find(l, "5 h", 1, true) then
		return "session", 5 * 3600
	end
	if string.find(l, "day", 1, true) or string.find(l, "daily", 1, true) or string.find(l, "24h", 1, true) then
		return "day", 86400
	end
	if string.find(l, "month", 1, true) or string.find(l, "30d", 1, true) then
		return "month", 30 * 86400
	end
	return nil, nil
end

-- Usage divided by how much of the window should have passed. 1 is exactly
-- on pace, above 1 ahead of it (the risky direction), below 1 behind.
local function pace(used: number, resets_at: number?, window_seconds: number?, now: number): number?
	if not resets_at or not window_seconds then
		return nil
	end
	local left = resets_at - now
	local elapsed = window_seconds - left
	if elapsed <= 0 or elapsed > window_seconds then
		return nil
	end
	local expected = elapsed / window_seconds
	if expected < 0.03 then
		return nil
	end
	return used / expected
end

local function copy_window(w: any, now: number): Window
	local label = tostring(w.label or "Usage")
	local kind, seconds = window_shape(label)
	local used = math.max(0, math.min(1, tonumber(w.used) or 0))
	local resets = tonumber(w.resets_at)
	return {
		label = label,
		used = used,
		resets_at = resets,
		kind = kind,
		window_seconds = seconds,
		pace = pace(used, resets, seconds, now),
	}
end

local function rank_window(w: Window): number
	local kind = w.kind or ""
	if kind == "session" then
		return 1
	elseif kind == "day" then
		return 2
	elseif kind == "week" then
		return 3
	elseif kind == "month" then
		return 4
	end
	return 9
end

local function add_unique(list: { string }, value: string)
	if not table.find(list, value) then
		table.insert(list, value)
	end
end

local function merge_windows(existing: Window, incoming: Window)
	-- The same upstream account probed through two agents can be a few
	-- seconds apart. Keep the conservative figure and the soonest reset.
	if incoming.used > existing.used then
		existing.used = incoming.used
		existing.pace = incoming.pace
	end
	if incoming.resets_at and (not existing.resets_at or incoming.resets_at < existing.resets_at) then
		existing.resets_at = incoming.resets_at
	end
	if not existing.kind then
		existing.kind = incoming.kind
		existing.window_seconds = incoming.window_seconds
	end
end

local function finish_account(account: Account)
	table.sort(account.windows, function(a: Window, b: Window): boolean
		local ra, rb = rank_window(a), rank_window(b)
		if ra ~= rb then
			return ra < rb
		end
		return a.label < b.label
	end)
	account.worst = 0
	for _, w in account.windows do
		account.worst = math.max(account.worst, w.used)
	end
end

-- Collect the host's `{ [harness] = { account } }` into provider pools.
-- `now` is Unix seconds; tests pass a fixed clock.
function M.collect(by_harness: { [string]: { any } }?, now: number?): { pools: { Pool }, total_accounts: number, total_windows: number, unreported: number }
	local at = now or os.time()
	local pooled: { [string]: { title: string, accounts: { [string]: Account}, order: { string } } } = {}
	local total_windows = 0
	local unreported = 0
	for harness, accounts in pairs(by_harness or {}) do
		for _, row in accounts or {} do
			local fam = family(harness, row.provider)
			local pool = pooled[fam]
			if not pool then
				pool = { title = TITLES[fam] or TITLES.other, accounts = {}, order = {} }
				pooled[fam] = pool
			end
			local email = tostring(row.email or "Signed in")
			-- A provider can only be deduplicated by a real identity. The
			-- generic "Signed in" rows stay separate rather than collapsing
			-- different people into one.
			local identity = lower(email)
			if identity == "" or identity == "signed in" then
				identity = `{harness}:{row.id or email}`
			end
			local key = `{fam}:{identity}`
			local account = pool.accounts[key]
			if not account then
				account = {
					key = key,
					email = email,
					plan = tostring(row.plan or ""),
					active = row.active == true,
					key_account = row.key == true,
					error = row.error,
					usage_fetched_at = tonumber(row.usage_fetched_at),
					harnesses = {},
					windows = {},
					worst = 0,
				}
				pool.accounts[key] = account
				table.insert(pool.order, key)
			end
			account.active = account.active or row.active == true
			account.key_account = account.key_account or row.key == true
			if account.plan == "" and row.plan then
				account.plan = tostring(row.plan)
			end
			if row.error then
				account.error = row.error
			end
			local fetched = tonumber(row.usage_fetched_at)
			if fetched and (not account.usage_fetched_at or fetched > account.usage_fetched_at) then
				account.usage_fetched_at = fetched
			end
			add_unique(account.harnesses, harness)
			-- Merge a window only when both its scope and its provider's own
			-- label match. Labels that differ ("Gemini Session" vs "Claude &
			-- GPT Session") are different quotas and stay side by side.
			for _, raw in row.windows or {} do
				local incoming = copy_window(raw, at)
				local found: Window? = nil
				local wanted = lower(incoming.label)
				for _, existing in account.windows do
					if lower(existing.label) == wanted then
						found = existing
						break
					end
				end
				if found then
					merge_windows(found, incoming)
				else
					table.insert(account.windows, incoming)
					total_windows += 1
				end
			end
		end
	end
	local pools: { Pool } = {}
	local total_accounts = 0
	for id, pool in pairs(pooled) do
		local out: Pool = { id = id, title = pool.title, accounts = {}, worst = 0, unreported = 0 }
		for _, key in pool.order do
			local account = pool.accounts[key]
			finish_account(account)
			if #account.windows == 0 then
				out.unreported += 1
				unreported += 1
			end
			out.worst = math.max(out.worst, account.worst)
			table.insert(out.accounts, account)
			total_accounts += 1
		end
		table.sort(out.accounts, function(a: Account, b: Account): boolean
			if a.active ~= b.active then
				return a.active
			end
			if a.worst ~= b.worst then
				return a.worst > b.worst
			end
			return lower(a.email) < lower(b.email)
		end)
		table.insert(pools, out)
	end
	table.sort(pools, function(a: Pool, b: Pool): boolean
		local ra, rb = ORDER[a.id] or ORDER.other, ORDER[b.id] or ORDER.other
		if ra ~= rb then
			return ra < rb
		end
		return a.title < b.title
	end)
	return { pools = pools, total_accounts = total_accounts, total_windows = total_windows, unreported = unreported }
end

return M
