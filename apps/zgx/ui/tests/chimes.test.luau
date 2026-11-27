-- The session chimes (screens/settings/chimes, issue #1437): "done" rings
-- when a turn finishes (its `turn` mark moving and standing a moment), never
-- for a chat turning "done" alone, which is only activity you haven't seen
-- and your own send makes too. Questions and failures ring at once.

local function chat(id: string, status: string?, turn: string?, fresh: boolean?): any
	return {
		id = id,
		title = id,
		device = "here",
		status = status,
		updated = 0,
		created = 0,
		turn = turn,
		turn_fresh = if fresh == nil then true else fresh,
	}
end

local function publish(app: any, chats: { any })
	app:set_state("sessions", {
		state = "ready",
		devices = {},
		projects = {},
		chats = chats,
		archived = {},
		places = { pinned = {}, sections = {} },
		places_ready = true,
	})
	app:frames(2)
end

local function played(app: any): { string }
	local out = {}
	for _, c in (app:state("settings_stub_calls") :: any) or {} do
		local sound = string.match(c, "^sound%.play:(.+)$")
		if sound then
			table.insert(out, sound)
		end
	end
	return out
end

-- Long enough for a finish to stand (chimes' SETTLE, 2 s).
local function settle(app: any)
	app:wait("#never-shown", { timeout_ms = 2400 })
	app:frames(3)
end

local function open(chats: { any }): any
	local app = mount("chimes_fixture.luau")
	publish(app, chats)
	return app
end

test("chimes: your own send never rings done", function()
	local app = open({ chat("a", nil, "t1") })
	-- The send lands: your message makes the chat unseen ("done") before the
	-- live row says it's working; the turn mark hasn't moved.
	publish(app, { chat("a", "done", "t1") })
	publish(app, { chat("a", "working", "t1") })
	-- A steer, or a queued message taken, as a turn ends: the end is
	-- published with the next turn already running.
	publish(app, { chat("a", "working", "t2") })
	-- A send that replaces the runtime: the old turn's end lands idle, then
	-- your turn starts a moment later.
	publish(app, { chat("a", "done", "t3") })
	publish(app, { chat("a", "working", "t3") })
	settle(app)
	expect(#played(app)).to_be(0)

	-- The turn your message started finishing: that rings, once.
	publish(app, { chat("a", "done", "t4") })
	expect(#played(app)).to_be(0)
	settle(app)
	expect(table.concat(played(app), ",")).to_be("done")
	settle(app)
	expect(#played(app)).to_be(1)
end)

test("chimes: a chat's first turn finishing rings done", function()
	local app = open({ chat("new", "working", nil) })
	publish(app, { chat("new", "done", "t1") })
	settle(app)
	expect(table.concat(played(app), ",")).to_be("done")
end)

test("chimes: an old row coming back, or a session row returning, never rings", function()
	local app = open({ chat("a", nil, "t1") })
	-- A sync brings back an end published long ago.
	publish(app, { chat("a", "done", "t2", false) })
	-- The row goes away (a host restarting) and comes back as it was.
	publish(app, { chat("a", nil, nil) })
	publish(app, { chat("a", nil, "t2") })
	settle(app)
	expect(#played(app)).to_be(0)
end)

test("chimes: questions and failures ring at once", function()
	local app = open({ chat("q", "working", "t1"), chat("f", "working", "t1") })
	publish(app, { chat("q", "input", "t1"), chat("f", "working", "t1") })
	expect(table.concat(played(app), ",")).to_be("request")
	publish(app, { chat("q", "input", "t1"), chat("f", "failed", "t1") })
	expect(table.concat(played(app), ",")).to_be("request,attention")
end)

test("chimes: Task completed off keeps done quiet, the rest still ring", function()
	local app = open({ chat("a", "working", "t1") })
	app:set_state("settings_sound_done", false)
	publish(app, { chat("a", "done", "t2") })
	settle(app)
	expect(#played(app)).to_be(0)
	publish(app, { chat("a", "input", "t2") })
	expect(table.concat(played(app), ",")).to_be("request")
end)

-- #1438: "done" means the chat's tail is there to read — it follows its
-- prefetch, and a tail that never lands still rings, bounded. (A test's
-- timers run on frame time — `app:frames(n)` is its 60th of a second — so
-- a wait of real seconds would blow past the bound: the wait's frames are
-- the countdown.)
test("chimes: done waits on the chat's tail, but not forever", function()
	local app = mount("chimes_fixture.luau")
	app:command("settings_stub.prefetch_hold")
	publish(app, { chat("a", "working", nil) })
	publish(app, { chat("a", "done", "t1") })
	-- The finish stands (SETTLE, ~120 frames) while its tail is still
	-- coming: no ring.
	app:frames(160)
	expect(#played(app)).to_be(0)
	-- The tail in hand: it rings (its settle look is rescheduled — a
	-- real-clock `ui.now()` gap the frame clock fills in ~120 more frames).
	app:command("settings_stub.prefetch_done")
	app:frames(140)
	expect(table.concat(played(app), ",")).to_be("done")
end)

test("chimes: a tail that never lands still rings, bounded", function()
	local app = mount("chimes_fixture.luau")
	app:command("settings_stub.prefetch_hold")
	publish(app, { chat("a", "working", nil) })
	publish(app, { chat("a", "done", "t1") })
	app:frames(160)
	expect(#played(app)).to_be(0)
	-- Past the settle plus the prefetch's bound (2 s + 4 s ≈ 360 frames):
	-- the finish rings rather than being swallowed.
	app:frames(240)
	expect(table.concat(played(app), ",")).to_be("done")
end)
