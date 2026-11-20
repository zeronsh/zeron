-- Scrolling up a long working chat from its end passes through every
-- message: each step moves what shows by the step, and the person's prompt
-- between the last two replies comes into view on the way. (Nov 10, beta.4:
-- scrolling up from the end of one chat skipped past a message in one frame;
-- not reproduced here, these hold the ways it was looked for. The window's
-- build-ahead and a synced chat's republishing: src/transcript_scroll_tests.rs.)
-- On chat_fixture's `long_chat` (ui/long_chat_fixture.luau): replies of
-- markdown, tables, thoughts and work, far taller than the list's estimate.

local function view(app: any): (number, number)
	for line in app:tree("#transcript"):gmatch("[^\n]+") do
		local y, h = line:match("#transcript scroll .-%-?%d+,(%-?%d+) %d+x(%d+)")
		if y then
			return tonumber(y) :: number, tonumber(h) :: number
		end
	end
	error("no transcript")
end

-- Each text in the view, by its words: its top.
local function shown(app: any): { [string]: number }
	local top, h = view(app)
	local out = {}
	for line in app:tree("#transcript"):gmatch("[^\n]+") do
		local words, y = line:match('text "(.-)" %-?%d+,(%-?%d+) %d+x%d+')
		local at = tonumber(y)
		if words and at and at >= top and at < top + h then
			out[words] = at
		end
	end
	return out
end

-- How far what shows moved (the median of the texts seen both times).
local function moved(before: { [string]: number }, after: { [string]: number }): number?
	local deltas: { number } = {}
	for words, y in before do
		local now = after[words]
		if now then
			table.insert(deltas, now - y)
		end
	end
	table.sort(deltas)
	return deltas[math.ceil(#deltas / 2)]
end

local function settle(app: any)
	app:wait("#never-shown", { timeout_ms = 250 })
	app:frames(10)
end

local function open(command: string): any
	local app = mount("chat_fixture.luau")
	app:resize(960, 700)
	app:frames(20)
	app:command(command)
	app:command("go_s7")
	app:frames(30)
	settle(app)
	return app
end

-- Up from the end by `dy` a frame, a trackpad's (`mouse`: a wheel's notches).
local function scroll_up_through(app: any, dy: number, mouse: boolean?)
	local skips = {}
	local seen = false
	local step: number? = nil
	for i = 1, math.ceil(2400 / dy) do
		local before = shown(app)
		for words in before do
			seen = seen or string.find(words, "three and one sounds like", 1, true) ~= nil
		end
		app:wheel("#transcript", { dy = -dy, mouse = mouse })
		app:frames(1)
		local d = moved(before, shown(app))
		-- (A wheel's notch moves as far as the list says a line is: the first
		-- step says how far.)
		step = step or d
		if d == nil or step == nil or math.abs(d - step) > 1 or d <= 0 then
			table.insert(skips, `step {i}: moved {d}`)
		end
	end
	expect(table.concat(skips, "; ")).to_be("")
	expect(seen).to_be(true)
end

for _, case in { { name = "a trackpad's small steps", dy = 8 }, { name = "a trackpad's steps", dy = 24 }, { name = "a flick", dy = 80 } } do
	test(`a long chat scrolled up from its end passes through every message: {case.name}`, function()
		local app = open("long_chat")
		scroll_up_through(app, case.dy)
	end)
end

test("a long chat scrolled up from its end passes through every message: a mouse wheel", function()
	local app = open("long_chat")
	scroll_up_through(app, 1, true)
end)

-- Its last prompt sent from the composer, its reply streamed in at the end
-- (the runway and the send's glide), then read up from the end.
test("a long chat scrolled up from its end after a turn sent here passes through every message", function()
	local app = open("long_chat_sent")
	app:type("role:textbox", "three and one sounds like the most optional option")
	app:key("Enter")
	app:frames(40)
	settle(app)
	for _ = 1, 140 do
		app:command("long_chat_streams")
		app:frames(2)
	end
	settle(app)
	settle(app)
	scroll_up_through(app, 24)
end)
