-- Going back to a chat puts the reader exactly where they were: the same
-- message at the same height in the view, or the end if they were at the
-- end. Also when the chat comes back as the engine sends it after a while
-- away: its newest rows first, its whole history a moment later
-- (`openingTail`; the chat fixture's `tail_first` and `history`).

-- The first question in view (its top inside or above the view's top, its
-- foot below it) and how far below the view's top it starts.
local function place(app: any): string
	local top, h = 0, 0
	local best: string? = nil
	local best_y = 0
	for line in app:tree("#transcript"):gmatch("[^\n]+") do
		local y, sh = line:match("#transcript scroll.- %-?%d+,(%-?%d+) %d+x(%d+)")
		if y then
			top, h = tonumber(y) :: number, tonumber(sh) :: number
		end
		local id, ry, rh = line:match("#(u%d+) article .- %-?%d+,(%-?%d+) %d+x(%d+)")
		if id then
			local y0, y1 = tonumber(ry) :: number, (tonumber(ry) :: number) + (tonumber(rh) :: number)
			if y1 > top and y0 < top + h and (best == nil or y0 < best_y) then
				best, best_y = id, y0
			end
		end
	end
	return if best then `{best} at {best_y - top}` else "nothing"
end

-- Whether the last question shows (the chat at its end).
local function at_end(app: any): boolean
	return app:has('#transcript >> "Question number 30: how does part 30 work?"')
end

-- Real time for the transcript's own waits (it notes its place 150 ms
-- after a scroll, and settles a restored place over 80 ms).
local function settle(app: any)
	app:wait("#never-shown", { timeout_ms = 250 })
	app:frames(10)
end

local function open(): any
	local app = mount("chat_fixture.luau")
	app:resize(960, 700)
	app:frames(40)
	settle(app)
	return app
end

local function away(app: any)
	app:command("go_s2")
	app:frames(20)
	settle(app)
end

local function back(app: any)
	app:command("go_s1")
	app:frames(20)
	settle(app)
end

test("a chat scrolled up comes back to the same message at the same height", function()
	local app = open()
	app:scroll("#transcript", -1250)
	app:frames(20)
	settle(app)
	local before = place(app)
	expect(at_end(app)).to_be(false)
	away(app)
	back(app)
	expect(place(app)).to_be(before)
end)

test("a chat scrolled up comes back there when its newest rows come first", function()
	local app = open()
	app:command("tail_first")
	-- Within the newest rows the engine sends first (the last eight turns).
	app:scroll("#transcript", -900)
	app:frames(20)
	settle(app)
	local before = place(app)
	expect(at_end(app)).to_be(false)
	away(app)
	back(app)
	app:command("history")
	app:frames(20)
	settle(app)
	expect(place(app)).to_be(before)
	-- And it stays there.
	app:frames(30)
	settle(app)
	expect(place(app)).to_be(before)
end)

test("a chat scrolled far up comes back there once its history arrives", function()
	local app = open()
	app:command("tail_first")
	-- Above the newest rows the engine sends first.
	app:scroll("#transcript", -2400)
	app:frames(20)
	settle(app)
	local before = place(app)
	away(app)
	back(app)
	-- (Until the rest comes: not at the end as though it had been left there.)
	app:command("history")
	app:frames(20)
	settle(app)
	expect(place(app)).to_be(before)
	-- Its place is kept: away and back again (the whole chat now), the same.
	away(app)
	back(app)
	app:command("history")
	app:frames(20)
	settle(app)
	expect(place(app)).to_be(before)
end)

-- Opened at its end, then all the way to its top: an offset back at the 0
-- the view was made with, so its rest is a value the rest already held
-- (`ui.debounced`'s `every`), and the place must still be noted.
test("a chat opened at its end and scrolled to its very top comes back at its top", function()
	local app = open()
	expect(at_end(app)).to_be(true)
	app:scroll("#transcript", -100000)
	app:frames(20)
	settle(app)
	local before = place(app)
	expect(string.match(before, "^u1 at ") ~= nil).to_be(true)
	away(app)
	back(app)
	expect(place(app)).to_be(before)
	expect(at_end(app)).to_be(false)
end)

test("a chat left at its end comes back at its end when its newest rows come first", function()
	local app = open()
	app:command("tail_first")
	expect(at_end(app)).to_be(true)
	away(app)
	back(app)
	expect(at_end(app)).to_be(true)
	app:command("history")
	app:frames(20)
	settle(app)
	expect(at_end(app)).to_be(true)
end)
