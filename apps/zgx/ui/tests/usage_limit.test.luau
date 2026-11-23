-- A thread its provider's usage limit stopped (its session's `limited`,
-- ds/usage_limit): waiting for the reset ("Waiting for limit · resets
-- 14:05", local) or running on another account meanwhile ("On <account> ·
-- <original> resets 14:05"), on the Board's card, Progress's row (its
-- countdown moving with its clock), the Map's node and hover card; and the
-- orchestrator's settings writing `crew.onLimit`.

local limit = require("@ds/usage_limit")

local function has(t: string, s: string): boolean
	return string.find(t, s, 1, true) ~= nil
end

-- (A test's own VM reads no catalog: the words themselves are read where
-- they're drawn, below.)
test("usage limit: the reset in local time; waiting or switched", function()
	local day = 1_800_040_000
	local reset = limit.local_at(day, 14, 5)
	expect(limit.clock(reset)).to_be("14:05")
	local waiting = { provider = "claude", account_label = "me@work.dev", until_at = reset, since = reset - 3600 }
	local switched = { provider = "claude", account_label = "me@work.dev", until_at = reset, since = reset - 3600,
		switched_to = { account_slot = "s2", account_label = "me@home.dev", at = reset - 3000 } }
	expect(limit.waiting({ limited = waiting })).to_be(true)
	expect(limit.waiting({ limited = switched })).to_be(false)
	expect(limit.switched(switched)).to_be(true)
	expect(limit.waiting({})).to_be(false)
	expect(limit.of({ limited = "nope" })).to_be(nil)
	-- No countdown once past, or with no reset known.
	expect(limit.left(waiting, reset + 1)).to_be("")
	expect(limit.left({ since = 0 }, reset)).to_be("")
end)

-- The Board.

local function board(): any
	local app = mount("orch_board_columns_fixture.luau")
	app:resize(1440, 900)
	app:frames(20)
	app:command("fixture.limited")
	app:frames(20)
	return app
end

test("usage limit: a Board card says its thread waits for the limit, and till when; one switched, where it runs", function()
	local app = board()
	local now = app:tree("#cols-now") :: string
	expect(has(now, "Sidebar rows drag between sections")).to_be(true)
	expect(has(now, "Waiting for limit · resets 14:05")).to_be(true)
	expect(has(now, "On me@home.dev · me@work.dev resets 14:05")).to_be(true)
	-- (Its glyph, before its words.)
	expect(app:has(".orch-card-limited")).to_be(true)
	-- A card whose thread isn't limited says nothing of it.
	expect(select(2, string.gsub(now, "resets 14:05", ""))).to_be(2)
end)

-- Progress.

local function progress(): any
	local app = mount("orch_progress_fixture.luau")
	app:resize(1240, 1400)
	app:frames(10)
	app:command("fixture.limited")
	app:frames(10)
	return app
end

-- What a countdown reads `secs` before the reset.
local function left_words(secs: number): string
	local m = secs // 60
	if m < 60 then
		return `resets in {m}m`
	end
	return if m % 60 == 0 then `resets in {m // 60}h` else `resets in {m // 60}h {m % 60}m`
end

test("usage limit: Progress's row waits for the limit in its own state, its countdown moving with its clock", function()
	local app = progress()
	local reset = app:state("limit_reset") :: number
	local now = app:state("now") :: number
	local full = app:tree("#fx-full-th-limit") :: string
	expect(has(full, "Waiting for limit · resets 14:05")).to_be(true)
	expect(has(full, left_words(reset - now))).to_be(true)
	local strip = app:tree("#fx-strip-th-limit") :: string
	expect(has(strip, "Waiting for limit · resets 14:05")).to_be(true)
	expect(has(strip, left_words(reset - now))).to_be(true)
	-- A minute on (the clock it already has), a minute less.
	app:command("fixture.later")
	app:frames(5)
	expect(has(app:tree("#fx-full-th-limit") :: string, left_words(reset - now - 60))).to_be(true)
end)

test("usage limit: Progress's row of a thread switched says where it runs meanwhile", function()
	local app = progress()
	local full = app:tree("#fx-full-th-switch") :: string
	expect(has(full, "On me@home.dev · me@work.dev resets 14:05")).to_be(true)
	expect(has(app:tree("#fx-strip-th-switch") :: string, "On me@home.dev · me@work.dev resets 14:05")).to_be(true)
end)

-- The Map.

local function map(): any
	local app = mount("orch_map_fixture.luau")
	app:resize(1440, 2400)
	app:command("map_only")
	app:set_state("control_map_force_pivot", "orchestrator")
	app:command("show_all")
	app:command("card_limited")
	app:command("map_force")
	app:frames(90)
	app:wait(400)
	app:frames(20)
	return app
end
local function hover(app: any, id: string): string
	app:set_state("control_map_force_hover", id)
	app:frames(3)
	app:hover(".orch-force-anchor-box")
	expect(app:wait(".orch-force-card", { timeout_ms = 1500 })).to_be(true)
	app:wait(700)
	app:frames(10)
	return app:tree(".orch-force-card") :: string
end

test("usage limit: the Map's node wears the limited glyph, its hover card says till when", function()
	local app = map()
	-- (The limited node's glyph over its ring, as asking or failed ones have.)
	expect(app:has(".orch-force-glyph")).to_be(true)
	local t = hover(app, "th-reins-notes")
	-- (Its facts apart, spaced: no typed separator on the card.)
	expect(has(t, "Waiting for limit")).to_be(true)
	expect(has(t, "resets 14:05")).to_be(true)
	expect(has(t, "Limited")).to_be(true)
	expect(has(t, "resets in")).to_be(true)
	expect(string.find(t, "·", 1, true)).to_be(nil)
end)

test("usage limit: the Map's hover card of a thread switched says where it runs", function()
	local app = map()
	local t = hover(app, "th-reins-win")
	-- (Its facts apart, spaced: no typed separator on the card.)
	expect(has(t, "On me@home.dev")).to_be(true)
	expect(has(t, "me@work.dev resets 14:05")).to_be(true)
	expect(string.find(t, "·", 1, true)).to_be(nil)
	-- (Still at work, on the other account.)
	expect(has(t, "Working")).to_be(true)
end)

-- The sidebar.

test("usage limit: a sidebar row stopped by its limit shows the limited glyph, its tooltip till when", function()
	local app = mount("shell_fixture.luau")
	app:resize(1200, 900)
	app:frames(30)
	app:command("fixture.limited")
	app:frames(30)
	local side = app:tree("#shell-sidebar") :: string
	expect(has(side, 'button "Tidy the tests')).to_be(true)
	expect(has(side, '"Waiting for limit · resets 14:05"')).to_be(true)
end)

-- The orchestrator's settings.

local function settings(): any
	local app = mount("orchestrator_fixture.luau")
	app:resize(1440, 1400)
	app:frames(30)
	app:command("orch.settings")
	app:frames(30)
	return app
end
local ACCOUNT = '#orch-settings-on-limit >> role:checkbox "When limited, continue on another account"'
local HARNESS = '#orch-settings-on-limit >> role:checkbox "…or another harness or model"'

test("usage limit: Settings' checkboxes, off by default, the second only with the first, write crew.onLimit", function()
	local app = settings()
	expect(app:has(ACCOUNT)).to_be(true)
	expect(app:has(HARNESS)).to_be(true)
	expect(app:state("orch_set_on_limit_account")).to_be(false)
	expect(app:state("orch_set_on_limit_harness")).to_be(false)
	-- (The second stands disabled while the first is off: a click does nothing.)
	expect(has(app:tree(HARNESS) :: string, "disabled")).to_be(true)
	app:click(HARNESS)
	app:frames(5)
	expect(app:state("orch_set_on_limit_harness")).to_be(false)
	-- On: another account; then the second, now enabled.
	app:click(ACCOUNT)
	app:frames(10)
	expect(app:state("orch_set_on_limit_account")).to_be(true)
	expect(has(string.match(app:tree(HARNESS) :: string, "[^\n]+") or "", "disabled")).to_be(false)
	app:click(HARNESS)
	app:frames(5)
	expect(app:state("orch_set_on_limit_harness")).to_be(true)
	app:click("#orch-settings-save")
	app:frames(10)
	expect(app:state("orch_calls")).to_contain("updateOrchestrator:crew={…},id=orc-orch")
	expect(app:state("orch_on_limit")).to_be("switchAccount=true,switchHarness=true")
end)

test("usage limit: Settings with the first alone sends another account only; nothing changed sends no crew", function()
	local app = settings()
	app:click("#orch-settings-save")
	app:frames(10)
	-- Nothing changed: no crew sent.
	expect(app:state("orch_on_limit")).to_be("")
	app:command("orch.settings")
	app:frames(30)
	app:click(ACCOUNT)
	app:frames(5)
	app:click("#orch-settings-save")
	app:frames(10)
	expect(app:state("orch_on_limit")).to_be("switchAccount=true,switchHarness=false")
end)
