-- The Map's "Show chats" (ds/orch_map_visibility): the rule itself, then
-- every view keeping it on ui/orch_map_fixture.luau (Code signing, on forge:
-- an idle crew thread, its card done, moved by the fixture's `sign_*`).

local Visibility = require("@ds/orch_map_visibility")

local NOW = 1800000000
local HOUR = 3600

local function ctx(mode: string, extra: any?): any
	local c = Visibility.context(extra or {}, mode)
	return c
end

local function chat(status: string?, ago: number, id: string?): any
	return { id = id or "c", status = status, updated = NOW - ago, created = NOW - ago - 60 }
end

test("map visibility: each mode, an idle chat", function()
	local idle = chat(nil, 10 * 60)
	expect(Visibility.visible(idle, NOW, ctx("all"))).to_be("show")
	expect(Visibility.visible(idle, NOW, ctx("active"))).to_be("hide")
	expect(Visibility.visible(idle, NOW, ctx("recent"))).to_be("show")
	-- Past the window: gone in "recent", still in "all".
	local old = chat(nil, 2 * HOUR)
	expect(Visibility.visible(old, NOW, ctx("recent"))).to_be("hide")
	expect(Visibility.visible(old, NOW, ctx("all"))).to_be("show")
	-- The default is "recent", an hour.
	expect(Visibility.DEFAULT).to_be("recent")
	expect(Visibility.WINDOW).to_be(HOUR)
end)

test("map visibility: a recent one fades through the window's last part, then hides", function()
	local c = ctx("recent")
	local v1, a1 = Visibility.visible(chat(nil, 20 * 60), NOW, c)
	expect(v1).to_be("show")
	expect(a1).to_be(1)
	local v2, a2 = Visibility.visible(chat(nil, 40 * 60), NOW, c)
	expect(v2).to_be("fade")
	expect(a2 < 1 and a2 > Visibility.FADE_MIN).to_be(true)
	local _, a3 = Visibility.visible(chat(nil, 55 * 60), NOW, c)
	expect(a3 < a2).to_be(true)
	expect(a3 >= Visibility.FADE_MIN).to_be(true)
	expect(Visibility.visible(chat(nil, 60 * 60), NOW, c)).to_be("hide")
end)

test("map visibility: working, asking, unread, an open ask or a card in Review always shows", function()
	for _, mode in Visibility.MODES do
		local c = ctx(mode, {
			asks = {
				{ id = "a", from_chat = "asks", state = "open" },
				{ id = "b", from_chat = "answered", state = "answered" },
				{ id = "d", from_chat = "later", state = "open", deferred_until = os.time() + 3600 },
			},
			items = {
				{ id = "i", state = "review", threads = { "review" } },
				{ id = "j", state = "done", threads = { "done-card" } },
			},
			orchestrators = { { id = "orc" }, { id = "brain", kind = "brain_dump" } },
		})
		for _, st in { "working", "waiting", "input", "approval", "done", "failed" } do
			expect(`{mode} {st} {Visibility.visible(chat(st, 5 * HOUR), NOW, c)}`).to_be(`{mode} {st} show`)
		end
		for _, id in { "asks", "review", "orc", "brain" } do
			expect(`{mode} {id} {Visibility.visible(chat(nil, 5 * HOUR, id), NOW, c)}`).to_be(`{mode} {id} show`)
		end
		-- Answered, put off, or a card not in Review: nothing pending.
		if mode ~= "all" then
			for _, id in { "answered", "later", "done-card" } do
				expect(`{mode} {id} {Visibility.visible(chat(nil, 5 * HOUR, id), NOW, c)}`).to_be(`{mode} {id} hide`)
			end
		end
	end
	-- Not heard yet: shown; archived: never.
	expect(Visibility.visible(nil, NOW, ctx("active"))).to_be("show")
	expect(Visibility.visible({ id = "x", archived = true, status = "working" }, NOW, ctx("all"))).to_be("hide")
end)

test("map visibility: new activity brings a hidden chat back at once", function()
	local c = ctx("recent")
	local row = chat(nil, 3 * HOUR)
	expect(Visibility.visible(row, NOW, c)).to_be("hide")
	-- A message: it moved now.
	row.updated = NOW
	expect(Visibility.visible(row, NOW, c)).to_be("show")
	-- In "active", a turn starting.
	local idle = chat(nil, 10)
	expect(Visibility.visible(idle, NOW, ctx("active"))).to_be("hide")
	idle.status = "working"
	expect(Visibility.visible(idle, NOW, ctx("active"))).to_be("show")
end)

-- On the Map.

local SIGN = "Code signing"

local function open(): any
	local app = mount("orch_map_fixture.luau")
	app:resize(1440, 2400)
	app:frames(30)
	return app
end

local function view(app: any, variant: string)
	app:command(`map_{variant}`)
	app:frames(20)
end

local function shows(app: any): boolean
	return string.find(app:tree(".orch-map") :: string, SIGN, 1, true) ~= nil
end

local function mode(app: any, m: string)
	app:set_state("ds/orch_map_visibility.luau::control_map_show", m)
	app:frames(20)
end

test("map visibility: every view hides an idle chat in Active only, shows it in All", function()
	local app = open()
	for _, v in { "graph", "topology", "timeline", "machines" } do
		view(app, v)
		mode(app, "all")
		expect(`{v} all {shows(app)}`).to_be(`{v} all true`)
		mode(app, "active")
		expect(`{v} active {shows(app)}`).to_be(`{v} active false`)
		-- What works still shows, and so does what asks (Orchestrator
		-- screen, awaiting input).
		expect(`{v} {string.find(app:tree(".orch-map") :: string, "Windows installer", 1, true) ~= nil}`).to_be(`{v} true`)
		expect(`{v} {string.find(app:tree(".orch-map") :: string, "Orchestrator screen", 1, true) ~= nil}`).to_be(`{v} true`)
	end
end)

test("map visibility: in the default mode, a chat idle past the window hides, and a message brings it back", function()
	local app = open()
	view(app, "topology")
	mode(app, "recent")
	expect(shows(app)).to_be(true)
	app:command("sign_old")
	app:frames(20)
	expect(shows(app)).to_be(false)
	app:command("sign_message")
	app:frames(20)
	expect(shows(app)).to_be(true)
end)

test("map visibility: unread shows for UNREAD_FOR after it last moved, then as any finished chat", function()
	for _, st in { "done", "failed" } do
		expect((Visibility.visible(chat(st, Visibility.UNREAD_FOR * 0.9), NOW, ctx("recent")))).to_be("show")
		expect((Visibility.visible(chat(st, Visibility.UNREAD_FOR * 0.9), NOW, ctx("active")))).to_be("show")
		expect((Visibility.visible(chat(st, Visibility.UNREAD_FOR + 2 * HOUR), NOW, ctx("recent")))).to_be("hide")
		expect((Visibility.visible(chat(st, Visibility.UNREAD_FOR + 2 * HOUR), NOW, ctx("active")))).to_be("hide")
		expect((Visibility.visible(chat(st, Visibility.UNREAD_FOR + 2 * HOUR), NOW, ctx("all")))).to_be("show")
	end
	-- Working or asking: always, however long.
	expect((Visibility.visible(chat("working", 100 * HOUR), NOW, ctx("active")))).to_be("show")
	expect((Visibility.visible(chat("input", 100 * HOUR), NOW, ctx("active")))).to_be("show")
end)

test("map visibility: an archived orchestrator's chat isn't kept on the Map", function()
	local c = ctx("recent", { orchestrators = { { id = "gone", archived = true }, { id = "here" } } })
	expect((Visibility.visible(chat(nil, 5 * HOUR, "gone"), NOW, c))).to_be("hide")
	expect((Visibility.visible(chat(nil, 5 * HOUR, "here"), NOW, c))).to_be("show")
end)

test("map visibility: a crew thread with no chat row shows while new, and in All", function()
	expect(Visibility.missing(NOW - 10 * 60, NOW, ctx("recent"))).to_be("show")
	expect(Visibility.missing(NOW - 10 * 60, NOW, ctx("active"))).to_be("show")
	expect(Visibility.missing(NOW - 2 * HOUR, NOW, ctx("recent"))).to_be("hide")
	expect(Visibility.missing(nil, NOW, ctx("active"))).to_be("hide")
	expect(Visibility.missing(NOW - 200 * HOUR, NOW, ctx("all"))).to_be("show")
	-- Through the fleet: an old crew row without a chat is left off; a new one stays.
	local model = require("@ds/orch_map_model")
	local world = {
		here = "d",
		devices = { { id = "d", name = "d", here = true, online = true } },
		chats = { o = { id = "o", status = "working", updated = NOW } },
		show = "recent",
		clock = NOW,
		now = {
			orchestrators = { { id = "o", hue = 1 } },
			crew = {
				{ orchestrator_id = "o", chat_id = "old", joined = NOW - 5 * HOUR },
				{ orchestrator_id = "o", chat_id = "new", joined = NOW - 60 },
			},
		},
	}
	local f = model.fleet(world)
	expect(f.threads.old).to_be(nil)
	expect(f.threads.new ~= nil).to_be(true)
end)

test("map visibility: a turn, unread output, an open ask or a Review card brings it back in Active only", function()
	for _, cmd in { "sign_turn", "sign_unread", "sign_ask", "sign_review" } do
		local app = open()
		view(app, "machines")
		mode(app, "active")
		expect(`{cmd} before {shows(app)}`).to_be(`{cmd} before false`)
		app:command(cmd)
		app:frames(20)
		expect(`{cmd} after {shows(app)}`).to_be(`{cmd} after true`)
	end
end)

test("map visibility: the Map's menu has Show chats, the choice kept", function()
	local first = open()
	expect(first:state("ds/orch_map_visibility.luau::control_map_show")).to_be("recent")
	first:click('#control-widget-map >> role:button "Widget menu"')
	first:frames(10)
	expect(first:has('"Show chats"')).to_be(true)
	expect(first:has('role:menuitemcheckbox "Active + recently finished"')).to_be(true)
	expect(first:has('role:menuitemcheckbox "All"')).to_be(true)
	first:click('role:menuitemcheckbox "Active only"')
	first:frames(20)
	expect(first:state("ds/orch_map_visibility.luau::control_map_show")).to_be("active")
	local app = open()
	expect(app:state("ds/orch_map_visibility.luau::control_map_show")).to_be("active")
end)
