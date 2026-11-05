-- Setup's order, run inside a mounted app (the test VM has `ui` only
-- there): the scenarios of tests/onboarding_order.test.luau, and what
-- failed, one line each, in #result ("ok" when nothing did). Here, beside
-- app.luau, as a mounted script loads modules from its own directory down.

local state = require("@screens/onboarding/state")
local actions = require("@screens/onboarding/actions")
local api = require("@account/api")

-- Zeron isn't here (a test host has no `account.*`): what setup tells it
-- goes nowhere.
local function nowhere(): any
	return { after = function() end }
end
api.onboarded = nowhere :: any
api.choose_org = nowhere :: any

local failures: { string } = {}
local current = ""

local function same(a: any, b: any): boolean
	if type(a) ~= "table" or type(b) ~= "table" then
		return a == b
	end
	if #a ~= #b then
		return false
	end
	for i = 1, #a do
		if a[i] ~= b[i] then
			return false
		end
	end
	return true
end

local function show(v: any): string
	if type(v) == "table" then
		return "{" .. table.concat(v, ", ") .. "}"
	end
	return tostring(v)
end

local function expect(actual: any)
	local function check(wanted: any)
		if not same(actual, wanted) then
			table.insert(failures, `{current}: got {show(actual)}, wanted {show(wanted)}`)
		end
	end
	return { to_be = check, to_equal = check }
end

local function test(name: string, body: () -> ())
	current = name
	local ok, err = pcall(body)
	if not ok then
		table.insert(failures, `{name}: {err}`)
	end
end

type Run = {
	path: string,
	name_done: boolean?,
	org_done: boolean?,
	org_admin: boolean?,
	passed: { string }?,
}

-- A run, as `signed_in` (or `skip_sign_in`) leaves it once Zeron answered.
local function run(r: Run): string
	state.path:set(r.path :: any)
	state.name_done:set(r.name_done == true)
	state.org_done:set(r.org_done == true)
	state.org_admin:set(r.org_admin == true)
	state.claimed:set(false)
	state.device_done:set({})
	state.step:set("signin")
	actions.start_run(r.passed or {})
	return state.step:peek()
end

-- Every step Continue visits from `from` up to done.
local function forward(from: string): { string }
	state.step:set(from :: any)
	local seen = { from }
	for _ = 1, 12 do
		if state.step:peek() == "done" then
			break
		end
		actions.next()
		table.insert(seen, state.step:peek())
	end
	return seen
end

-- Every step Back visits from `from` until it stops.
local function backward(from: string): { string }
	state.step:set(from :: any)
	local seen = { from }
	for _ = 1, 12 do
		local before = state.step:peek()
		actions.back()
		if state.step:peek() == before then
			break
		end
		table.insert(seen, state.step:peek())
	end
	return seen
end

test("a new account opens on the look and goes through every step", function()
	expect(run { path = "signed_in" }).to_be("appearance")
	expect(forward("appearance")).to_equal({ "appearance", "username", "card", "org", "invite", "github", "providers", "done" })
end)

test("a new account goes back past its username and organization, never to Sign in", function()
	run { path = "signed_in" }
	-- (Made in the run: what naming it leaves.)
	actions.org_made("org_test", "Halcyon Labs")
	expect(backward("done")).to_equal({ "done", "providers", "github", "invite", "card", "appearance" })
end)

test("claiming a username goes to the card, making an organization to inviting", function()
	run { path = "signed_in" }
	state.step:set("username")
	actions.claimed_username({} :: any)
	expect(state.step:peek()).to_be("card")
	state.step:set("org")
	actions.org_made("org_test", "Halcyon Labs")
	expect(state.step:peek()).to_be("invite")
	actions.next()
	expect(state.step:peek()).to_be("github")
end)

test("an account with a username and its own organization still invites", function()
	expect(run { path = "signed_in", name_done = true, org_done = true, org_admin = true }).to_be("appearance")
	expect(forward("appearance")).to_equal({ "appearance", "card", "invite", "github", "providers", "done" })
	expect(backward("done")).to_equal({ "done", "providers", "github", "invite", "card", "appearance" })
end)

test("a member of someone else's organization passes over inviting", function()
	run { path = "signed_in", name_done = true, org_done = true, org_admin = false }
	expect(forward("card")).to_equal({ "card", "github", "providers", "done" })
	expect(backward("github")).to_equal({ "github", "card", "appearance" })
end)

test("a returning account opens on its first page left, and Continue goes through every step from there", function()
	-- (The card and inviting finished before: the user's case.)
	local first = run {
		path = "signed_in",
		name_done = true,
		org_done = true,
		org_admin = true,
		passed = { "appearance", "card", "invite" },
	}
	expect(first).to_be("github")
	expect(backward("github")).to_equal({ "github", "invite", "card", "appearance" })
	expect(forward("appearance")).to_equal({ "appearance", "card", "invite", "github", "providers", "done" })
end)

test("a page finished on this Mac counts at the start too", function()
	state.path:set("signed_in")
	state.name_done:set(true)
	state.org_done:set(true)
	state.org_admin:set(true)
	state.device_done:set({ appearance = true })
	state.step:set("signin")
	actions.start_run({})
	expect(state.step:peek()).to_be("card")
	expect(backward("card")).to_equal({ "card", "appearance" })
end)

test("the username turning out to be had moves on to the card", function()
	run { path = "signed_in" }
	state.step:set("username")
	actions.has_username()
	expect(state.step:peek()).to_be("card")
end)

test("the organization turning out to be had moves on by its role", function()
	run { path = "signed_in", name_done = true }
	state.step:set("org")
	actions.work_in({ id = "o", organization_id = "o", name = "Halcyon Labs", role = "admin" })
	actions.next()
	expect(state.step:peek()).to_be("invite")
	run { path = "signed_in", name_done = true }
	state.step:set("org")
	actions.work_in({ id = "o", organization_id = "o", name = "Halcyon Labs", role = "member" })
	actions.next()
	expect(state.step:peek()).to_be("github")
end)

test("without an account: the look, the agents, done, and back to Sign in", function()
	expect(run { path = "skipped" }).to_be("appearance")
	expect(forward("appearance")).to_equal({ "appearance", "providers", "done" })
	expect(backward("done")).to_equal({ "done", "providers", "appearance", "signin" })
end)

test("an account that finished every page opens on done, and Back still works", function()
	local first = run {
		path = "signed_in",
		name_done = true,
		org_done = true,
		org_admin = true,
		passed = { "appearance", "card", "invite", "github", "providers" },
	}
	expect(first).to_be("done")
	expect(backward("done")).to_equal({ "done", "providers", "github", "invite", "card", "appearance" })
end)

test("the dots count every step and never move", function()
	run { path = "signed_in" }
	local _, all = state.position("appearance")
	expect(all).to_be(8)
	run { path = "signed_in", name_done = true, org_done = true, org_admin = false, passed = { "appearance", "card" } }
	local n, count = state.position("github")
	expect(count).to_be(8)
	expect(n).to_be(6)
	run { path = "skipped" }
	local _, few = state.position("providers")
	expect(few).to_be(3)
	local last = state.position("done")
	expect(last).to_be(3)
end)

return ui.text {
	id = "result",
	if #failures == 0 then "ok" else table.concat(failures, "\n"),
}
