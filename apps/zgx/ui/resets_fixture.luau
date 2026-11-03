-- Settings → Providers with a Codex account at its limit and two resets
-- banked, mounted on its own with agents made up here (no engine in a test
-- host): what tests/codex_resets.test.luau drives. account/agents' calls
-- are stubbed as the host answers them (src/agents.rs): `use_reset` puts
-- the account's reset "using", and `resets.answer` brings what Codex said
-- (`resets_next`: "reset", "nothing", "gone" or "failed:<why>"), the
-- account as the host then publishes it (the limits started over: its
-- windows at 0 and that reset gone from the count). What was asked, in
-- order, is `resets_calls`. Here, beside app.luau, as a mounted script
-- loads modules from its own directory down.
--
--   resets.answer      Codex answers the reset under way
--   resets.one         one reset banked
--   resets.none        none banked
--   resets.fresh       a fresh usage probe after a reset: the real numbers

ui.theme.strict("warn")
local ds = require("@ds")
ds.setup()
-- Addresses plain to start (hiding is the app's default): the tests read
-- them, and turn hiding on themselves.
ds.PrivateText.hidden:set(false)
-- Settings' host calls (the page's settings dialog, its toasts).
require("./settings_stub")
local agents = require("@account/agents")
local registry = require("@account/registry") :: any
registry.load = function() end
local providers = require("@app/screens/settings/providers")

local now = os.time()
-- When the used-up session starts over: two hours on.
local SESSION_RESETS = now + 2 * 3600

local function fresh_codex(): any
	return {
		id = "cx1",
		email = "ada@example.com",
		plan = "Plus",
		session = 1,
		week = 0.64,
		active = true,
		session_resets = SESSION_RESETS,
		week_resets = now + 4 * 86400,
		banked = 2,
		-- Soonest to expire first, as the host sorts them.
		resets = {
			{ id = "c1", title = "Full reset (Weekly + 5 hr)", scope = "Week and session", expires = now + 2 * 86400 },
			{ id = "c2", title = "Full reset (Weekly + 5 hr)", scope = "Week and session", expires = now + 10 * 86400 },
		},
	}
end

local function agent(id: string, name: string, mark: string, accounts: { any }): any
	return {
		id = id,
		name = name,
		mark = mark,
		installed = true,
		can_install = true,
		enabled = true,
		version = "1.0.0",
		installing = false,
		login = "idle",
		accounts = accounts,
	}
end

local codex = fresh_codex()
local model: { any } = {
	-- A Claude account at its limit too: only Codex banks resets.
	agent("claude-code", "Claude Code", "claude", {
		{ id = "cl1", email = "bo@example.com", plan = "Max", session = 1, week = 0.3, active = true },
	}),
	agent("codex", "Codex", "openai", { codex }),
}

local function publish()
	local list = {}
	for _, a in model do
		local item = table.clone(a)
		local accounts = {}
		for _, acc in a.accounts do
			local one = table.clone(acc)
			if acc.resets then
				one.resets = table.clone(acc.resets)
			end
			table.insert(accounts, one)
		end
		item.accounts = accounts
		table.insert(list, item)
	end
	agents.now:set({ state = "ready", list = list } :: any)
end

local said: { string } = {}
local calls = ui.state("resets_calls", "")
local function note(what: string)
	table.insert(said, what)
	calls:set(table.concat(said, ","))
end
local next_word = ui.state("resets_next", "reset")
-- The reset under way: which credit.
local pending: string? = nil

local stub = agents :: any
stub.load = function() end
stub.target = function() end
stub.use_reset = function(account: string, credit: string)
	note(`use_reset:{account}:{credit}`)
	if account ~= codex.id then
		return
	end
	pending = credit
	codex.reset = { state = "using" }
	publish()
end
stub.dismiss_reset = function(account: string)
	note(`dismiss_reset:{account}`)
	if account == codex.id then
		codex.reset = nil
		publish()
	end
end

local function spend(credit: string)
	local left = {}
	for _, r in codex.resets do
		if r.id ~= credit then
			table.insert(left, r)
		end
	end
	codex.resets = left
	codex.banked = math.max(0, codex.banked - 1)
end

ui.command {
	id = "resets.answer",
	title = "Codex answers the reset under way",
	run = function()
		local credit = pending
		if not credit then
			return
		end
		pending = nil
		local word = next_word:peek()
		local why = string.match(word, "^failed:(.+)$")
		if why then
			codex.reset = { state = "failed", why = why }
		elseif word == "reset" then
			codex.session, codex.week = 0, 0
			spend(credit)
			codex.reset = { state = "reset" }
		elseif word == "gone" then
			spend(credit)
			codex.reset = { state = "gone" }
		else
			codex.reset = { state = word }
		end
		publish()
	end,
}
ui.command {
	id = "resets.one",
	title = "One reset banked",
	run = function()
		codex.resets = { codex.resets[1] }
		codex.banked = 1
		publish()
	end,
}
ui.command {
	id = "resets.none",
	title = "None banked",
	run = function()
		codex.resets = {}
		codex.banked = 0
		codex.reset = nil
		publish()
	end,
}
ui.command {
	id = "resets.fresh",
	title = "A fresh usage probe",
	run = function()
		codex.session, codex.week = 0.02, 0.11
		publish()
	end,
}

publish()

return ui.scroll {
	id = "page",
	w_full = true,
	h_full = true,
	-- (The window's own opaque ground, as the gallery's stage draws it.)
	bg = "$zeron.window",
	ui.column { w_full = true, px = "$space.6", py = "$space.6", providers.view() },
}
