-- A chat opened at one of its messages (route.open_session(id, { message })):
-- what a Control room card or a Needs-you ask waiting on a decision does,
-- with the ask's turn (the message that asked). The transcript goes to that
-- message, near its top, and lights it a moment (ds/transcript `focus`), not
-- to its latest; without a message it opens as ever. On chat_fixture's long
-- chat (s1: thirty questions, each answered).

-- Where the view is: its top and height.
local function view(app: any): (number, number)
	for line in app:tree("#transcript"):gmatch("[^\n]+") do
		local y, h = line:match("#transcript scroll.- %-?%d+,(%-?%d+) %d+x(%d+)")
		if y then
			return tonumber(y) :: number, tonumber(h) :: number
		end
	end
	error("no transcript")
end

-- Where message `id` starts below the view's top (nil: not laid out).
local function top_of(app: any, id: string): number?
	local top = view(app)
	for line in app:tree("#transcript"):gmatch("[^\n]+") do
		local y = line:match(`#{id} article .- %-?%d+,(%-?%d+) %d+x%d+`)
		if y then
			return (tonumber(y) :: number) - top
		end
	end
	return nil
end

-- Whether message `id` stands in the view (its top within it).
local function in_view(app: any, id: string): string
	local _, h = view(app)
	local y = top_of(app, id)
	if y == nil then
		return `{id} not laid out`
	end
	return if y >= 0 and y < h - 120 then `{id} in view` else `{id} at {y}`
end

local function at_end(app: any): boolean
	return app:has('#transcript >> "Question number 30: how does part 30 work?"')
end

-- The messages lit since the last move (`go`), and whether `id`'s light
-- shows now.
local seen_lit: { [string]: boolean } = {}
local function look(app: any)
	for id in app:tree("#transcript"):gmatch("#transcript%-flash%-([%w%-]+)") do
		seen_lit[id] = true
	end
end
local function lit(_app: any, id: string): boolean
	return seen_lit[id] == true
end
local function lit_now(app: any, id: string): boolean
	return app:has(`#transcript-flash-{id}`)
end

local function settle(app: any)
	app:wait("#never-shown", { timeout_ms = 250 })
	app:frames(10)
end

-- The light's whole run (held, then fading), in real time.
local function after_the_light(app: any)
	app:wait("#never-shown", { timeout_ms = 2600 })
	app:frames(10)
end

local function open(): any
	local app = mount("chat_fixture.luau")
	app:resize(960, 700)
	app:frames(40)
	settle(app)
	return app
end

local function go(app: any, command: string)
	seen_lit = {}
	app:command(command)
	for _ = 1, 30 do
		app:frames(1)
		look(app)
	end
	settle(app)
end

test("open at: a long chat opened at an early message shows that message, lit, not its latest", function()
	local app = open()
	go(app, "go_s2")
	go(app, "go_s1_at_u3")
	expect(in_view(app, "u3")).to_be("u3 in view")
	expect(at_end(app)).to_be(false)
	expect(lit(app, "u3")).to_be(true)
	expect(app:has('role:button ~"Jump to latest"')).to_be(true)
	-- The light goes; the view stays on the message (nothing holding the end
	-- takes it back there).
	after_the_light(app)
	expect(lit_now(app, "u3")).to_be(false)
	expect(in_view(app, "u3")).to_be("u3 in view")
	expect(at_end(app)).to_be(false)
end)

test("open at: a reply asked at is gone to as a prompt is", function()
	local app = open()
	go(app, "go_s2")
	go(app, "go_s1_at_a3")
	expect(lit(app, "a3")).to_be(true)
	-- (Its question just above it, the reply at the view's top.)
	expect(at_end(app)).to_be(false)
	expect(app:has('#transcript >> "Part 3 works like this."')).to_be(true)
end)

test("open at: a chat opened without a message still opens at its latest", function()
	local app = open()
	go(app, "go_s2")
	go(app, "go_s1")
	expect(at_end(app)).to_be(true)
	expect(next(seen_lit) == nil).to_be(true)
end)

test("open at: the chat already open jumps to the message, and again to another", function()
	local app = open()
	go(app, "go_s1")
	expect(at_end(app)).to_be(true)
	go(app, "go_s1_at_u3")
	expect(in_view(app, "u3")).to_be("u3 in view")
	expect(lit(app, "u3")).to_be(true)
	expect(at_end(app)).to_be(false)
	go(app, "go_s1_at_u20")
	expect(in_view(app, "u20")).to_be("u20 in view")
	expect(lit(app, "u20")).to_be(true)
	expect(lit(app, "u3")).to_be(false)
	-- Asked again, the same one lights again.
	after_the_light(app)
	expect(lit_now(app, "u20")).to_be(false)
	go(app, "go_s1_at_u20")
	expect(lit(app, "u20")).to_be(true)
end)

test("open at: a jump stays put through a resize of the window", function()
	local app = open()
	go(app, "go_s1_at_u3")
	app:resize(900, 640)
	app:frames(20)
	settle(app)
	expect(in_view(app, "u3")).to_be("u3 in view")
	expect(at_end(app)).to_be(false)
end)

test("open at: a message not in the rows yet is gone to as the chat's history brings it", function()
	local app = open()
	go(app, "go_s2")
	app:command("tail_first")
	go(app, "go_s1_at_u3")
	-- (Its newest rows only, meanwhile: the latest shows.)
	expect(at_end(app)).to_be(true)
	expect(next(seen_lit) == nil).to_be(true)
	seen_lit = {}
	app:command("history")
	for _ = 1, 30 do
		app:frames(1)
		look(app)
	end
	settle(app)
	expect(in_view(app, "u3")).to_be("u3 in view")
	expect(lit(app, "u3")).to_be(true)
end)

test("open at: scrolling before the history comes lets the jump go", function()
	local app = open()
	go(app, "go_s2")
	app:command("tail_first")
	go(app, "go_s1_at_u3")
	for _ = 1, 5 do
		app:wheel("#transcript", { dy = -40 })
		app:frames(1)
	end
	settle(app)
	seen_lit = {}
	app:command("history")
	for _ = 1, 30 do
		app:frames(1)
		look(app)
	end
	settle(app)
	expect(next(seen_lit) == nil).to_be(true)
	expect(top_of(app, "u3")).to_be(nil)
end)

test("open at: a message that isn't there leaves the chat at its latest, quietly", function()
	local app = open()
	go(app, "go_s2")
	go(app, "go_s1_at_nope")
	expect(at_end(app)).to_be(true)
	expect(next(seen_lit) == nil).to_be(true)
	expect(app:has("#chat-session")).to_be(true)
end)
