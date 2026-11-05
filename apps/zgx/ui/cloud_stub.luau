-- Cloud, Billing and GitHub for Cloud, answered here as the host would (no
-- engine or edge in a test host): account/cloud, account/billing and
-- account/github's calls. Required by ui/settings_stub.luau (the shell's
-- fixture) and ui/chat_fixture.luau; tests/cloud_billing.test.luau and
-- tests/chat_where_cloud.test.luau drive it.
--
-- The account has no Cloud until `cloud_stub.offer`. What was asked, in
-- order, is `cloud_stub_calls` (a state). `ui.command`s change the data as
-- the edge would on its own:
--   cloud_stub.offer           the account has Cloud, on, GitHub connected,
--                              acme/atlas and two more repositories reachable
--   cloud_stub.reload          Cloud's status read again
--   cloud_stub.no_repos        GitHub connected, no repositories chosen
--   cloud_stub.repos_fail      the next repositories read fails
--   cloud_stub.last_7_days     Settings → Cloud's usage over the last 7 days
--   cloud_stub.no_github       GitHub not connected (no repositories)
--   cloud_stub.purchase_lands  a Stripe page's purchase lands: 12,000 extra
--   cloud_stub.fail_next       the next Cloud or Billing call fails ("failed")

local auth = require("@account/auth")
local cloud = require("@account/cloud")
local billing = require("@account/billing")
local github = require("@account/github")

local calls = ui.state("cloud_stub_calls", {} :: { string })
local function log(entry: string)
	calls:push(entry)
end

local offered = ui.state("cloud_stub_offered", false)
local connected = ui.state("cloud_stub_github", true)
local fail = ui.state("cloud_stub_fail", false)

-- A promise already settled, as the host's settle: `:after` at once.
local function settled(value: any, word: string?): any
	local p = { done = true }
	function p.after(_self: any, ok: ((any) -> ())?, bad: ((any) -> ())?)
		if word then
			if bad then
				bad({ message = word })
			end
		elseif ok then
			ok(value)
		end
	end
	return p
end
local function answer(value: any): any
	if fail:peek() then
		fail:set(false)
		return settled(nil, "failed")
	end
	return settled(value)
end

local DEVICE = "cloud-account-1"
local state = "ready"
local function status(): cloud.Status
	if not offered:peek() then
		return { state = "off", available = false }
	end
	return {
		state = state,
		available = true,
		device = if state == "off" then nil else DEVICE,
		credits = 44095,
		awake = 0,
		max_awake = 5,
	}
end

local REPOS: { cloud.Repo } = {
	{ name = "acme/atlas", private = true, branch = "main", clone_url = "https://github.com/acme/atlas.git", pushed = 3000 },
	{ name = "acme/site", private = false, branch = "main", clone_url = "https://github.com/acme/site.git", pushed = 2000 },
	{ name = "octo/tools", private = false, branch = "trunk", clone_url = "https://github.com/octo/tools.git", pushed = 2500 },
}
-- What the next repositories read answers: "ok", "none" (connected, none
-- chosen), "fail" (the edge out of reach).
local repos_next = ui.state("cloud_stub_repos_next", "ok")

local c = cloud :: any
c.load = function()
	log("cloud.status")
	local r = answer(status())
	r:after(function(s: any)
		cloud.take(s)
	end, function(e: any)
		cloud.now:set { phase = "failed", repos_loading = false, failure = e.message }
	end)
end
c.load_repos = function(done: (() -> ())?)
	log("cloud.repos")
	local now = table.clone(cloud.now:peek()) :: any
	now.repos_loading = false
	now.repos_failure = nil
	if not connected:peek() then
		now.repos, now.repos_failure = {}, "github_not_connected"
	elseif repos_next:peek() == "none" then
		now.repos = {}
	elseif repos_next:peek() == "fail" then
		now.repos, now.repos_failure = nil, "failed"
	else
		now.repos = table.clone(REPOS)
	end
	cloud.now:set(now)
	if done then
		done()
	end
end
c.usage = function(from: string, to: string): any
	log(`cloud.usage:{from}:{to}`)
	-- Thirty days, three of them busy.
	local days = {}
	for i = 1, 30 do
		local credits = if i == 17 then 604 elseif i == 19 then 786 elseif i == 20 then 115 else 0
		table.insert(days, { day = string.format("2026-10-%02d", i), credits = credits })
	end
	return answer { days = days, credits = 1505, balance = 44095, available = true }
end
c.branches = function(repo: string, default: string?): any
	log(`cloud.branches:{repo}`)
	return answer {
		{ name = default or "main", current = true },
		{ name = "release/3.0", current = false },
	}
end

local plans: { billing.Plan } = {
	{ id = "base", name = "Base", price_cents = 2000, monthly_credits = 45000 },
	{ id = "pro", name = "Pro", price_cents = 5000, monthly_credits = 112500 },
	{ id = "max", name = "Max", price_cents = 20000, monthly_credits = 450000 },
}
local extra = 0
local function billing_status(): billing.Status
	return {
		available = true,
		plan = {
			id = "base",
			name = "Base",
			price_cents = 2000,
			monthly_credits = 45000,
			status = "active",
			-- (Nov 25, 2026, noon UTC.)
			period_end = 1795608000,
			cancel_at_period_end = false,
		},
		credits = { monthly = 44099, monthly_total = 45000, monthly_ends = 1795608000, extra = extra, total = 44099 + extra },
		plans = plans,
		pack = { price_cents = 1000, credits = 12000 },
		has_customer = true,
	}
end

local b = billing :: any
b.status = function(): any
	log("billing.status")
	return answer(billing_status())
end
b.subscribe = function(plan: string): any
	log(`billing.subscribe:{plan}`)
	return answer(true)
end
b.buy = function(packs: number): any
	log(`billing.buy:{packs}`)
	return answer(true)
end
b.manage = function(): any
	log("billing.portal")
	return answer(true)
end

local function publish_github()
	if connected:peek() then
		github.now:set({ phase = "connected", account = "octo-example", repos = { "acme/atlas", "acme/site", "octo/tools" } } :: any)
	else
		github.now:set({ phase = "idle", repos = {} } :: any)
	end
end
local g = github :: any
g.load = function()
	log("github.load")
	publish_github()
end
g.connect = function()
	log("github.connect")
	github.now:set({ phase = "waiting", code = "WDJB-MJHT", repos = {} } :: any)
end
g.cancel = function()
	log("github.cancel")
	publish_github()
end
g.open = function()
	log("github.open")
end
g.choose = function()
	log("github.choose")
end
g.disconnect = function()
	log("github.disconnect")
	connected:set(false)
	publish_github()
end

ui.command {
	id = "cloud_stub.offer",
	title = "The account has Cloud",
	run = function()
		offered:set(true)
		state = "ready"
		-- (Cloud is an account's: signed in, if the fixture isn't yet.)
		if auth.state:peek() ~= "signed_in" then
			auth.account:set({ state = "signed_in", env = "staging", user = { id = "user_ada", email = "ada@example.net", first_name = "Ada" } } :: any)
		end
		cloud.load()
		publish_github()
	end,
}
ui.command {
	id = "cloud_stub.no_repos",
	title = "GitHub connected, no repositories chosen for Cloud",
	run = function()
		repos_next:set("none")
	end,
}
ui.command {
	id = "cloud_stub.repos_fail",
	title = "The next repositories read fails",
	run = function()
		repos_next:set("fail")
	end,
}
ui.command {
	id = "cloud_stub.reload",
	title = "Cloud's status read again (as turning off is followed)",
	run = function()
		cloud.load()
	end,
}
-- (The usage range picked: Select's menu is the platform's, out of a test's
-- reach.)
ui.command {
	id = "cloud_stub.last_7_days",
	title = "Usage over the last 7 days",
	run = function()
		require("@app/screens/settings/cloud").range:set("7d")
	end,
}
ui.command {
	id = "cloud_stub.no_github",
	title = "GitHub isn't connected for Cloud",
	run = function()
		connected:set(false)
		publish_github()
		cloud.load_repos()
	end,
}
ui.command {
	id = "cloud_stub.purchase_lands",
	title = "A purchase on Stripe's page lands",
	run = function()
		extra += 12000
	end,
}
ui.command {
	id = "cloud_stub.fail_next",
	title = "The next Cloud or Billing call fails",
	run = function()
		fail:set(true)
	end,
}

return {}
