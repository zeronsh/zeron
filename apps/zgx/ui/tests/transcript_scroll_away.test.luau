-- Scrolled away from the end, the view stays where the person took it: it
-- isn't pulled back to the end. (Nov 14: "scrolling gets stuck to the
-- bottom; scroll away and it scrolls you back down".) Two ways it was:
-- - a scroll during a send's glide (the prompt rising to the view's top, or
--   a steer's waiting to land): the glide went on to the end over it;
-- - a scroll begun within moments of the view's size changing: the end
--   stayed held, and the next resize, however much later (the sidebar, a
--   panel, the window), went back to it.
-- On chat_fixture's `long_chat` (ui/long_chat_fixture.luau).

local function settle(app: any)
	app:wait("#never-shown", { timeout_ms = 250 })
	app:frames(10)
end

local function offset(app: any): number
	return app:state("chat.transcript.offset")
end

-- How far the view is from its end, measured by going there (which holds
-- the end again: only at the close of a test).
local function from_end(app: any): number
	local at = offset(app)
	app:scroll("#transcript", 1e6)
	app:frames(4)
	return offset(app) - at
end

local function open(): any
	local app = mount("chat_fixture.luau")
	app:resize(960, 700)
	app:frames(20)
	app:command("long_chat_sent")
	app:command("go_s7")
	app:frames(30)
	settle(app)
	return app
end

local function send(app: any)
	app:type("role:textbox", "three and one sounds like the most optional option")
	app:key("Enter")
end

local function scroll_up(app: any)
	for _ = 1, 10 do
		app:wheel("#transcript", { dy = -40 })
		app:frames(1)
	end
end

test("scrolled up while a send glides, the view stays where it was taken", function()
	local app = open()
	send(app)
	app:frames(3)
	scroll_up(app)
	settle(app)
	settle(app)
	expect(from_end(app) > 100).to_be(true)
end)

test("scrolled up just after a resize, a later resize leaves the view where it was", function()
	local app = open()
	send(app)
	app:frames(40)
	settle(app)
	for _ = 1, 60 do
		app:command("long_chat_streams")
		app:frames(2)
	end
	app:frames(10)
	app:resize(940, 700)
	app:frames(2)
	scroll_up(app)
	-- (It rests inside the resize's moment, then a while.)
	settle(app)
	app:wait("#never-shown", { timeout_ms = 800 })
	local at = offset(app)
	app:resize(900, 700)
	app:frames(10)
	settle(app)
	expect(math.abs(offset(app) - at) < 1).to_be(true)
	expect(from_end(app) > 100).to_be(true)
end)
