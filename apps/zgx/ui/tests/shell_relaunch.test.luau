-- A relaunch opens the main window where it was left: the session viewed
-- last (not the one most recently active), at once, with no step through
-- a new chat as the chats come; one archived or gone since gives way to
-- the most recent. A second `mount` is the next launch (what the first
-- saved, it finds). On the shell in ui/shell_fixture.luau, which starts
-- launched; each relaunch here undoes that and lets the chats come again.

local SIDE = "#shell-sidebar"
local LAUNCHED = "screens/shell/route.luau::shell_launch_opened"
local LAST = "screens/shell/route.luau::shell_last_route"

local function open(): any
	local app = mount("shell_fixture.luau")
	app:resize(1200, 800)
	app:frames(30)
	return app
end

-- The next launch, as the chats are about to come: they come with `arrive`
-- (a session newer than any, "Brand new", a4).
local function relaunch(): any
	local app = open()
	app:set_state(LAUNCHED, false)
	return app
end

local function history(app: any): { entries: { string }, at: number }
	return app:state("shell_route_history") :: any
end

test("a relaunch opens the session viewed last, even with another more recently active", function()
	local first = open()
	-- The oldest session, viewed last; then a while (its place saved).
	first:click(`{SIDE} >> role:button "Answer the review"`)
	first:frames(30)
	expect(first:state("shell_route_path")).to_be("/session/b2")
	expect(first:state("shell_last_route")).to_be("/session/b2")

	local app = relaunch()
	-- There from the start, before the chats settle it.
	expect(app:state("shell_route_path")).to_be("/session/b2")
	app:command("arrive")
	app:frames(10)
	expect(app:state("shell_launch_opened")).to_be(true)
	expect(app:state("shell_route_path")).to_be("/session/b2")
	-- No step through anywhere else: its one place in the history.
	expect(history(app).entries).to_equal({ "/session/b2" })
	expect(app:has("#session-header")).to_be(true)
	expect(app:text("#page-title")).to_contain("Answer the review")
end)

test("a relaunch from settings opens the session under it", function()
	local first = open()
	first:click(`{SIDE} >> role:button "Tidy the tests"`)
	first:frames(10)
	first:command("shell.settings")
	first:frames(30)
	expect(first:state("shell_route_path")).to_be("/settings/general")
	expect(first:state("shell_last_route")).to_be("/session/a3")

	local app = relaunch()
	app:command("arrive")
	app:frames(10)
	expect(app:state("shell_route_path")).to_be("/session/a3")
end)

test("a relaunch left on a new chat stays on a new chat", function()
	local first = open()
	first:click(`{SIDE} >> role:button "Tidy the tests"`)
	first:frames(10)
	first:command("shell.new_chat")
	first:frames(30)
	expect(first:state("shell_last_route")).to_be("/")

	local app = relaunch()
	app:command("arrive")
	app:frames(10)
	expect(app:state("shell_launch_opened")).to_be(true)
	expect(app:state("shell_route_path")).to_be("/")
end)

test("a relaunch whose session was archived since opens the most recent", function()
	local first = open()
	-- (z1: among the archived.)
	first:set_state(LAST, "/session/z1")
	first:frames(30)

	local app = relaunch()
	app:command("arrive")
	app:frames(10)
	expect(app:state("shell_route_path")).to_be("/session/a4")
	-- In place of the archived one: no step back to it.
	expect(history(app).entries).to_equal({ "/session/a4" })
end)

test("a relaunch whose session is gone opens the most recent, after a moment for it to arrive", function()
	local first = open()
	first:set_state(LAST, "/session/gone")
	first:frames(30)

	local app = relaunch()
	app:command("arrive")
	app:frames(10)
	-- (Not among the chats yet: waited for a moment.)
	expect(app:state("shell_route_path")).to_be("/session/gone")
	app:frames(150)
	expect(app:state("shell_launch_opened")).to_be(true)
	expect(app:state("shell_route_path")).to_be("/session/a4")
	expect(history(app).entries).to_equal({ "/session/a4" })
end)

test("a relaunch whose session arrives late stays on it", function()
	local first = open()
	-- (a4: not among the chats until `arrive`.)
	first:set_state(LAST, "/session/a4")
	first:frames(30)

	local app = relaunch()
	-- The chats come without it, then with it.
	app:command("work")
	app:frames(10)
	expect(app:state("shell_launch_opened")).to_be(false)
	app:command("arrive")
	app:frames(10)
	expect(app:state("shell_launch_opened")).to_be(true)
	app:frames(150)
	expect(app:state("shell_route_path")).to_be("/session/a4")
	expect(history(app).entries).to_equal({ "/session/a4" })
end)

test("a link followed as it launches wins over where it was left", function()
	local first = open()
	first:click(`{SIDE} >> role:button "Answer the review"`)
	first:frames(30)

	local app = relaunch()
	app:set_state("app.link", { seq = 1, chat = "a2" })
	app:frames(5)
	expect(app:state("shell_route_path")).to_be("/session/a2")
	app:command("arrive")
	app:frames(10)
	expect(app:state("shell_route_path")).to_be("/session/a2")
	-- And it's where the next launch opens.
	app:frames(30)
	expect(app:state("shell_last_route")).to_be("/session/a2")
end)

test("a first launch, nothing saved, opens the most recent", function()
	local app = relaunch()
	expect(app:state("shell_route_path")).to_be("/")
	app:command("arrive")
	app:frames(10)
	expect(app:state("shell_route_path")).to_be("/session/a4")
	expect(history(app).entries).to_equal({ "/session/a4" })
end)
