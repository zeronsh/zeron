-- Mark all done as read, from the force Map's space menu: every chat on it
-- finished and not yet read, none at work, asking or in Review; gone from
-- the Map at once, an Undo for a moment, then read (sessions.seen) on every
-- device. A chat read by opening it stays its window from then.

local Visibility = require("@ds/orch_map_visibility")

local function open(): any
	local app = mount("orch_map_fixture.luau")
	app:resize(1440, 2400)
	app:command("map_only")
	app:command("map_force")
	app:set_state("ds/orch_map_visibility.luau::control_map_show", "recent")
	app:frames(60)
	return app
end
local function ids(app: any): { [string]: boolean }
	app:command("map_mark_read_count")
	app:frames(1)
	local out = {}
	for _, id in (app:state("mark_read") or {}).ids or {} do
		out[id] = true
	end
	return out
end
local function on_map(app: any, text: string): boolean
	return string.find(app:tree(".orch-map") :: string, text, 1, true) ~= nil
end

test("mark done as read: the count is the finished, unread chats on the Map, none at work, asking or in Review", function()
	local app = open()
	local got = ids(app)
	local n = 0
	for _ in got do
		n += 1
	end
	expect(n >= 2).to_be(true)
	-- (Finished, unread: in; working, asking, Review: never.)
	expect(got["th-omarchy"]).to_be(true)
	expect(got["th-tally-ci"]).to_be(true)
	expect(got["th-ui-orch"]).to_be(nil)
	expect(got["th-wg-audit"]).to_be(nil)
	expect(got["th-reins-win"]).to_be(nil)
	-- (Its menu says how many: the space's, at a corner no node takes.)
	app:right_click(".orch-force-legend")
	app:frames(5)
	expect(app:has(`"Mark {n} done as read"`)).to_be(true)
	app:key("Escape")
end)

test("mark done as read: they go from the Map at once; Undo brings them back, nothing read; else read after the Undo's moment", function()
	local app = open()
	expect(on_map(app, "Rerun omarchy-update")).to_be(true)
	expect(on_map(app, "Orchestrator screen")).to_be(true)
	app:command("map_mark_read_do")
	app:frames(30)
	expect(on_map(app, "Rerun omarchy-update")).to_be(false)
	-- (Asking: untouched.)
	expect(on_map(app, "Orchestrator screen")).to_be(true)
	-- (Its toast, with Undo.)
	local toast = nil
	for _, t in app:state("ds_toasts") or {} do
		if t.action and t.action.run == "control.unread" then
			toast = t
		end
	end
	expect(toast ~= nil and toast.action.label == "Undo").to_be(true)
	app:command("map_mark_read_undo")
	app:frames(30)
	expect(on_map(app, "Rerun omarchy-update")).to_be(true)
	app:wait(6500)
	app:frames(5)
	expect(string.find(tostring(app:state("calls")), "seen:", 1, true)).to_be(nil)
	-- Again, no Undo: read once the moment's passed, and still off the Map.
	app:command("map_mark_read_do")
	app:frames(10)
	app:wait(6500)
	app:frames(10)
	local calls = tostring(app:state("calls"))
	expect(string.find(calls, "seen:th-omarchy", 1, true) ~= nil).to_be(true)
	expect(string.find(calls, "seen:th-ui-orch", 1, true)).to_be(nil)
	expect(on_map(app, "Rerun omarchy-update")).to_be(false)
	-- (Nothing left to mark: the item says so, and can't be chosen.)
	local left = ids(app)
	expect(next(left)).to_be(nil)
end)

test("mark done as read: a chat read by opening it stays its window from then, never gone at once", function()
	local NOW = 1800000000
	local ctx = Visibility.context({}, "recent", nil, { old = NOW - 60 }, {})
	-- (Finished hours ago, read a minute ago: shown.)
	local chat = { id = "old", updated = NOW - 5 * 3600 }
	expect((Visibility.visible(chat, NOW, ctx))).to_be("show")
	-- (Never read here: the window from its last move, gone.)
	expect((Visibility.visible({ id = "other", updated = NOW - 5 * 3600 }, NOW, ctx))).to_be("hide")
	-- (Put away by hand: gone, till it moves again.)
	local away = Visibility.context({}, "recent", nil, {}, { fresh = NOW - 10 })
	expect((Visibility.visible({ id = "fresh", status = "done", updated = NOW - 60 }, NOW, away))).to_be("hide")
	expect((Visibility.visible({ id = "fresh", status = "done", updated = NOW - 5 }, NOW, away))).to_be("show")
end)
