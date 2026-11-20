-- A chat left at its end comes back at its end (the latest), whatever came
-- meanwhile: replies landing while it was away, its rows measured anew (taller
-- than the list's estimate), its history arriving after its newest rows. One
-- left mid-scroll comes back to the same message at the same height. On the
-- chat fixture's s7 (tall replies) and s2.

-- The view's top and height.
local function view(app: any): (number, number)
	for line in app:tree("#transcript"):gmatch("[^\n]+") do
		local y, h = line:match("#transcript scroll .-%-?%d+,(%-?%d+) %d+x(%d+)")
		if y then
			return tonumber(y) :: number, tonumber(h) :: number
		end
	end
	error("no transcript")
end

-- Whether text `words` shows in the view (its line's top inside it).
local function shows(app: any, words: string): boolean
	local top, h = view(app)
	for line in app:tree("#transcript"):gmatch("[^\n]+") do
		if line:find(words, 1, true) then
			local y = tonumber(line:match(" %-?%d+,(%-?%d+) %d+x%d+"))
			if y and y >= top - 1 and y < top + h then
				return true
			end
		end
	end
	return false
end

-- The first question in view and how far below the view's top it starts.
local function place(app: any): string
	local top, h = view(app)
	local best, best_y = nil, 0
	for line in app:tree("#transcript"):gmatch("[^\n]+") do
		local id, ry, rh = line:match("#(tu%d+) article .- %-?%d+,(%-?%d+) %d+x(%d+)")
		if id then
			local y0 = tonumber(ry) :: number
			local y1 = y0 + (tonumber(rh) :: number)
			if y1 > top and y0 < top + h and (best == nil or y0 < best_y) then
				best, best_y = id, y0
			end
		end
	end
	return if best then `{best} at {best_y - top}` else "nothing"
end

local function settle(app: any)
	app:wait("#never-shown", { timeout_ms = 250 })
	app:frames(10)
end

local function open(): any
	local app = mount("chat_fixture.luau")
	app:resize(960, 700)
	app:frames(20)
	app:command("go_s7")
	app:frames(30)
	settle(app)
	return app
end

-- Up a way and back down to the bottom, by hand.
local function to_bottom_by_hand(app: any)
	app:scroll("#transcript", -900)
	app:frames(20)
	settle(app)
	for _ = 1, 6 do
		app:scroll("#transcript", 600)
		app:frames(5)
	end
	app:frames(20)
	settle(app)
end

local function away_and_back(app: any, meanwhile: { string }, via: string?)
	app:command(via or "go_s2")
	app:frames(20)
	settle(app)
	for _, c in meanwhile do
		app:command(c)
		app:frames(5)
	end
	app:command("go_s7")
	app:frames(30)
	settle(app)
end

-- At its end: its last reply's last paragraph in view, no way back to the latest.
local function at_end(app: any, last: string)
	expect(`{last} shows: {shows(app, last)}`).to_be(`{last} shows: true`)
	expect(app:has('role:button ~"Jump to latest"')).to_be(false)
end

for _, case in {
	{ name = "nothing came", meanwhile = {}, last = "Reply 20, paragraph 10", history = false },
	{ name = "a reply landed", meanwhile = { "reply_away" }, last = "Late reply 21, paragraph 12", history = false },
	{ name = "it opens with its newest rows first", meanwhile = { "tail_first" }, last = "Reply 20, paragraph 10", history = true },
	{ name = "a reply landed and it opens with its newest rows first", meanwhile = { "tail_first", "reply_away" }, last = "Late reply 21, paragraph 12", history = true },
} do
	test(`a chat scrolled to its bottom by hand comes back at its bottom: {case.name}`, function()
		local app = open()
		to_bottom_by_hand(app)
		at_end(app, "Reply 20, paragraph 10")
		away_and_back(app, case.meanwhile)
		at_end(app, case.last)
		if case.history then
			app:command("history")
			app:frames(30)
			settle(app)
			at_end(app, case.last)
		end
		-- And it stays there as its rows measure.
		app:frames(60)
		settle(app)
		at_end(app, case.last)
	end)
end

test("a chat of tall replies left mid-scroll comes back to the same message, a reply landing meanwhile", function()
	local app = open()
	app:scroll("#transcript", -3000)
	app:frames(20)
	settle(app)
	local before = place(app)
	expect(before).never.to_be("nothing")
	away_and_back(app, { "reply_away" })
	expect(place(app)).to_be(before)
	app:frames(60)
	settle(app)
	expect(place(app)).to_be(before)
end)

-- Away to a page that isn't a chat (a new chat, settings), and back.
for _, via in { "go_new", "go_home" } do
	for _, meanwhile in { {}, { "reply_away" }, { "tail_first", "reply_away" } } do
		test(`a chat scrolled to its bottom comes back at its bottom from {via} ({#meanwhile} things meanwhile)`, function()
			local app = open()
			to_bottom_by_hand(app)
			at_end(app, "Reply 20, paragraph 10")
			away_and_back(app, meanwhile, via)
			local last = if #meanwhile > 0 then "Late reply 21, paragraph 12" else "Reply 20, paragraph 10"
			if table.find(meanwhile, "tail_first") then
				app:command("history")
				app:frames(30)
				settle(app)
			end
			at_end(app, last)
			app:frames(60)
			settle(app)
			at_end(app, last)
		end)
	end
end

-- As the host opens a chat it no longer holds: the transcript emptied and
-- loading at once, the chat a moment later (and its newest rows first).
for _, meanwhile in { { "slow_open" }, { "slow_open", "reply_away" }, { "slow_open", "tail_first", "reply_away" } } do
	test(`a chat scrolled to its bottom comes back at its bottom as the host opens it ({table.concat(meanwhile, ", ")})`, function()
		local app = open()
		to_bottom_by_hand(app)
		at_end(app, "Reply 20, paragraph 10")
		away_and_back(app, meanwhile)
		app:wait('"Reply 1, paragraph 1"', { timeout_ms = 1000 })
		app:frames(30)
		settle(app)
		local last = if table.find(meanwhile, "reply_away") then "Late reply 21, paragraph 12" else "Reply 20, paragraph 10"
		if table.find(meanwhile, "tail_first") then
			app:command("history")
			app:frames(30)
			settle(app)
		end
		at_end(app, last)
		app:frames(60)
		settle(app)
		at_end(app, last)
	end)
end

test("a chat left mid-scroll comes back to the same message as the host opens it, a reply landing meanwhile", function()
	local app = open()
	app:scroll("#transcript", -3000)
	app:frames(20)
	settle(app)
	local before = place(app)
	away_and_back(app, { "slow_open", "reply_away" })
	app:wait("#never-shown", { timeout_ms = 600 })
	app:frames(30)
	settle(app)
	expect(place(app)).to_be(before)
end)

-- By trackpad: wheel steps up a way, then down past the bottom, as a hand
-- flicks it there.
test("a chat flicked to its bottom by trackpad comes back at its bottom, a reply landing meanwhile", function()
	local app = open()
	for _ = 1, 10 do
		app:wheel("#transcript", { dy = -90 })
		app:frames(1)
	end
	app:frames(20)
	settle(app)
	for _ = 1, 30 do
		app:wheel("#transcript", { dy = 120 })
		app:frames(1)
	end
	app:frames(30)
	settle(app)
	at_end(app, "Reply 20, paragraph 10")
	away_and_back(app, { "reply_away" })
	at_end(app, "Late reply 21, paragraph 12")
end)

test("a chat flicked to its bottom by trackpad comes back at its bottom, nothing meanwhile", function()
	local app = open()
	for _ = 1, 10 do
		app:wheel("#transcript", { dy = -90 })
		app:frames(1)
	end
	app:frames(20)
	settle(app)
	for _ = 1, 30 do
		app:wheel("#transcript", { dy = 120 })
		app:frames(1)
	end
	app:frames(30)
	settle(app)
	at_end(app, "Reply 20, paragraph 10")
	away_and_back(app, {})
	at_end(app, "Reply 20, paragraph 10")
end)
