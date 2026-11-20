-- A chat left mid-scroll and come back to later comes back to the same
-- message at the same height: after other chats were opened, a new chat, the
-- chat reopened from the engine (its rows let go), a reply
-- landing or a turn running meanwhile, and across a relaunch. On the chat
-- fixture's s7 (replies far taller than the list's row estimate).

local function view(app: any): (number, number)
	for line in app:tree("#transcript"):gmatch("[^\n]+") do
		local y, h = line:match("#transcript scroll .-%-?%d+,(%-?%d+) %d+x(%d+)")
		if y then
			return tonumber(y) :: number, tonumber(h) :: number
		end
	end
	error("no transcript")
end

-- The first text block in view (a reply's paragraph, a question's words) and
-- how far below the view's top it starts.
local function place(app: any): string
	local top, h = view(app)
	local best, best_y = nil, 0
	for line in app:tree("#transcript"):gmatch("[^\n]+") do
		local words, y = line:match('text "([^"]-)…?" %-?%d+,(%-?%d+) %d+x%d+')
		local at = tonumber(y)
		if words and at and at >= top and at < top + h and (best == nil or at < best_y) then
			best, best_y = words, at
		end
	end
	return if best then `{best} at {best_y - top}` else "nothing"
end

local function settle(app: any)
	app:wait("#never-shown", { timeout_ms = 250 })
	app:frames(10)
end

local function go(app: any, command: string)
	app:command(command)
	app:frames(20)
	settle(app)
end

-- s7 opened and scrolled well up into it, by trackpad.
local function open_mid(): (any, string)
	local app: any = mount("chat_fixture.luau")
	app:resize(960, 700)
	app:frames(20)
	go(app, "go_s7")
	for _ = 1, 25 do
		app:wheel("#transcript", { dy = -120 })
		app:frames(1)
	end
	app:frames(20)
	settle(app)
	local here = place(app)
	expect(here).never.to_be("nothing")
	expect(app:has('role:button ~"Jump to latest"')).to_be(true)
	return app, here
end

local function back_there(app: any, here: string)
	app:frames(30)
	settle(app)
	expect(place(app)).to_be(here)
	-- And it stays there as its rows measure.
	app:frames(60)
	settle(app)
	expect(place(app)).to_be(here)
end

test("mid-scroll, back after several other chats", function()
	local app, here = open_mid()
	for _, c in { "go_s2", "go_s1", "go_s4", "go_s5", "go_s8", "go_s3" } do
		go(app, c)
	end
	go(app, "go_s7")
	back_there(app, here)
end)

test("mid-scroll, back from a new chat", function()
	local app, here = open_mid()
	go(app, "go_new")
	go(app, "go_s7")
	back_there(app, here)
end)

test("mid-scroll, back after it was let go and reopened from the engine, a reply landing", function()
	local app, here = open_mid()
	go(app, "slow_open")
	go(app, "tail_first")
	go(app, "go_s2")
	go(app, "reply_away")
	go(app, "go_s7")
	app:wait("#never-shown", { timeout_ms = 500 })
	go(app, "history")
	back_there(app, here)
end)

test("mid-scroll while a turn runs, back after it ended", function()
	local app, here = open_mid()
	go(app, "start_reply")
	go(app, "stream_words")
	go(app, "go_s2")
	go(app, "reply_away")
	go(app, "go_s7")
	back_there(app, here)
end)

test("mid-scroll, back after a relaunch", function()
	local first, here = open_mid()
	-- (A while at rest: its place noted, as a person waits before quitting.)
	first:wait("#never-shown", { timeout_ms = 400 })
	first:frames(10)
	local app = mount("chat_fixture.luau")
	app:resize(960, 700)
	app:frames(40)
	settle(app)
	go(app, "go_s7")
	back_there(app, here)
end)

test("mid-scroll, left for another chat, back after a relaunch", function()
	local first, here = open_mid()
	go(first, "go_s2")
	first:wait("#never-shown", { timeout_ms = 400 })
	first:frames(10)
	local app = mount("chat_fixture.luau")
	app:resize(960, 700)
	app:frames(40)
	settle(app)
	go(app, "go_s7")
	back_there(app, here)
end)

test("at its end, back after a relaunch at its end", function()
	local first = mount("chat_fixture.luau")
	first:resize(960, 700)
	first:frames(20)
	go(first, "go_s7")
	expect(first:has('role:button ~"Jump to latest"')).to_be(false)
	first:wait("#never-shown", { timeout_ms = 400 })
	first:frames(10)
	local app = mount("chat_fixture.luau")
	app:resize(960, 700)
	app:frames(40)
	settle(app)
	go(app, "go_s7")
	app:frames(60)
	settle(app)
	expect(app:has('role:button ~"Jump to latest"')).to_be(false)
	expect(app:has('~"Reply 20, paragraph 10"')).to_be(true)
end)
