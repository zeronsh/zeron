-- A chat opened again where it was read shows its newest rows first, then its
-- history arrives above them: the message read stays where it was, from the
-- frame the rows come in, not only once a correction lands. (Nov 11, beta.5:
-- going back to a chat, the view showed older rows for a few frames, then
-- jumped back; the transcript scrolled to the message first, by estimates of
-- rows not measured yet, though the list kept it in place.) At the very top
-- of the rows shown, where the list keeps nothing in place, the view is put
-- back once laid out. On chat_fixture's long working chat.

-- Where message `id` starts below the view's top, if it's laid out.
local function top_of(app: any, id: string): number?
	local escaped = string.gsub(id, "%-", "%%-")
	local view: number? = nil
	for line in app:tree("#transcript"):gmatch("[^\n]+") do
		local y = line:match("#transcript scroll .-%-?%d+,(%-?%d+) %d+x%d+")
		if y then
			view = tonumber(y)
		end
		local row = line:match(`#{escaped} article .- %-?%d+,(%-?%d+) %d+x%d+`)
		if row and view then
			return (tonumber(row) :: number) - (view :: number)
		end
	end
	return nil
end

-- The first message starting at least `below` points down the view.
local function first_below(app: any, below: number): string
	local view = 0
	for line in app:tree("#transcript"):gmatch("[^\n]+") do
		local y = line:match("#transcript scroll .-%-?%d+,(%-?%d+) %d+x%d+")
		if y then
			view = tonumber(y) :: number
		end
		local id, row = line:match("#([%w%-]+) article .- %-?%d+,(%-?%d+) %d+x%d+")
		if id and (tonumber(row) :: number) - view >= below then
			return id
		end
	end
	error("no message in view")
end

local function settle(app: any)
	app:wait("#never-shown", { timeout_ms = 250 })
	app:frames(10)
end

-- The long chat, read up into, left, and opened again with its newest rows
-- first (their history held back until `history`).
local function opened_again(up: number): any
	local app = mount("chat_fixture.luau")
	app:resize(960, 700)
	app:frames(20)
	app:command("long_chat_cross")
	app:command("go_s7")
	app:frames(30)
	settle(app)
	for _ = 1, up do
		app:wheel("#transcript", { dy = -40 })
		app:frames(1)
	end
	settle(app)
	settle(app)
	app:command("go_s2")
	app:frames(20)
	settle(app)
	app:command("tail_first")
	app:command("go_s7")
	app:frames(30)
	settle(app)
	return app
end

test("history arriving above the message read leaves it where it was, from the first frame", function()
	local app = opened_again(30)
	local id = first_below(app, 60)
	local was = top_of(app, id)
	expect(was).never.to_be(nil)
	app:command("history")
	-- (Read before any frame's timers run: what the rows' arrival itself did.)
	expect(`{id} at {top_of(app, id)}`).to_be(`{id} at {was}`)
	for _ = 1, 6 do
		app:frames(1)
		expect(`{id} at {top_of(app, id)}`).to_be(`{id} at {was}`)
	end
	settle(app)
	expect(`{id} at {top_of(app, id)}`).to_be(`{id} at {was}`)
	expect(app:has('role:button ~"Jump to latest"')).to_be(true)
end)

test("history arriving above the top of the rows shown puts the view back on the message read", function()
	local app = opened_again(30)
	-- Up to the first of the newest rows, where the list keeps nothing in place.
	for _ = 1, 60 do
		app:wheel("#transcript", { dy = -80 })
		app:frames(1)
	end
	settle(app)
	settle(app)
	local id = first_below(app, 0)
	local was = top_of(app, id)
	expect(was).never.to_be(nil)
	app:command("history")
	settle(app)
	settle(app)
	expect(`{id} at {top_of(app, id)}`).to_be(`{id} at {was}`)
end)
