-- Tab and ⇧Tab through a long chat bring each row's control they focus
-- into view, clear of the title bar's fade over the transcript's top and of
-- the composer floating over its foot (ds/transcript's `scroll_padding`).
-- On the chat fixture's long chat (30 questions, each reply with a tool row).
--
-- (A transcript made before its composer measured: its end held a tick
-- after each change of size, which in a test landed among the keys, once
-- moved a focused row back above the view: ds/transcript lets the end go
-- once focus comes into the list.) A chat switched away and back: ⇧Tab goes
-- on through every row, past those built (zgx f7a8b003), never round to the
-- composer.

local function box(app: any, target: string): (number, number, number, number)
	local line = string.match(app:tree(target), "[^\n]+") or ""
	local x, y, w, h = line:match(" (%-?%d+),(%-?%d+) (%d+)x(%d+)")
	return tonumber(x) or 0, tonumber(y) or 0, tonumber(w) or 0, tonumber(h) or 0
end

-- The focused node in the transcript: its line, top and bottom; nil if focus
-- is elsewhere.
local function focused(app: any): (string?, number, number)
	for line in app:tree("#transcript"):gmatch("[^\n]+") do
		if line:find(" focused", 1, true) then
			local y, h = line:match(" %-?%d+,(%-?%d+) %d+x(%d+)")
			return line, tonumber(y) or 0, (tonumber(y) or 0) + (tonumber(h) or 0)
		end
	end
	return nil, 0, 0
end

local function long_chat(): any
	local app = mount("chat_fixture.luau")
	app:resize(960, 700)
	app:frames(40)
	return app
end

-- From the composer, ⇧Tab up through the rows, then Tab back down: each
-- focused row clear of both bands, and none landing outside the transcript.
local function walk(app: any)
	local _, top = box(app, "#transcript")
	local _, composer = box(app, "#chat-composer")
	expect(composer > top).to_be(true)
	local seen = 0
	local function check(how: string)
		local line, y, bottom = focused(app)
		if line then
			seen += 1
			expect(`{how}: {line} clear of the top {y >= top + 24}`).to_be(`{how}: {line} clear of the top true`)
			expect(`{how}: {line} clear of the composer {bottom <= composer}`).to_be(`{how}: {line} clear of the composer true`)
		end
	end
	app:click("#chat-composer >> role:textbox")
	app:frames(5)
	for i = 1, 14 do
		app:key("shift+Tab")
		app:frames(8)
		check(`⇧Tab {i}`)
	end
	for i = 1, 14 do
		app:key("Tab")
		app:frames(8)
		check(`Tab {i}`)
	end
	-- (Every step a row's, none round to the composer's controls, but the
	-- last Tab: down past the newest row, back to the composer.)
	expect(seen).to_be(27)
end

test("Tab and ⇧Tab through a long chat bring each focused row clear of the title fade and the composer", function()
	walk(long_chat())
end)

test("after a chat switched away and back, ⇧Tab goes on up through every row", function()
	local app = long_chat()
	app:command("go_s2")
	app:frames(20)
	app:command("go_s1")
	app:frames(40)
	walk(app)
end)
