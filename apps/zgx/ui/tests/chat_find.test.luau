-- Find in a chat (screens/chat/find): it searches the messages' text, not
-- the window's nodes, so a long reply's words far from the view are found;
-- the current match's run is built and scrolled into view, its words
-- marked; Enter and the arrows step; Escape closes.

local function long_chat(): any
	local app = mount("chat_fixture.luau")
	app:resize(960, 700)
	app:command("go_s6")
	app:frames(20)
	app:command("long_reply")
	app:frames(40)
	app:wait("#never-shown", { timeout_ms = 60 })
	app:frames(10)
	return app
end

local function settle(app: any)
	for _ = 1, 4 do
		app:wait("#never-shown", { timeout_ms = 20 })
		app:frames(5)
	end
end

local function tally(app: any): string
	return app:tree("#chat-find"):match('text "(%d+ of %d+)"') or app:tree("#chat-find"):match('text "(No results)"') or ""
end

-- The y of the first line holding `words` in the transcript's tree, or nil.
local function y_of(app: any, words: string): number?
	for line in app:tree("#transcript"):gmatch("[^\n]+") do
		if line:find(words, 1, true) then
			return tonumber(line:match(" %-?%d+,(%-?%d+) "))
		end
	end
	return nil
end

test("a match far up a long reply is found, built and scrolled into view", function()
	local app = long_chat()
	-- At the reply's end: Part 3 is far above, its run held (not built).
	expect(y_of(app, "Part 3 explains")).to_be(nil)
	app:command("find_open")
	settle(app)
	expect(app:has("#chat-find")).to_be(true)
	app:type('role:textbox "Find"', "section 3,")
	settle(app)
	expect(tally(app)).to_be("1 of 1")
	local y = y_of(app, "Part 3 explains")
	expect(y ~= nil).to_be(true)
	local first = app:tree("#transcript"):match("[^\n]+") or ""
	local top, h = first:match(" %-?%d+,(%-?%d+) %d+x(%d+)")
	top, h = tonumber(top), tonumber(h)
	expect((y :: number) >= (top :: number) and (y :: number) < (top :: number) + (h :: number)).to_be(true)
	expect(app:error()).to_be(nil)
end)

test("Enter and the arrows step through the matches; Escape closes", function()
	local app = long_chat()
	app:command("find_open")
	settle(app)
	app:type('role:textbox "Find"', "fn part_9")
	settle(app)
	-- part_9, part_90 … part_99: eleven, the newest current.
	expect(tally(app)).to_be("11 of 11")
	app:key("Enter")
	settle(app)
	expect(tally(app)).to_be("1 of 11")
	app:key("ArrowUp")
	settle(app)
	expect(tally(app)).to_be("11 of 11")
	app:type('role:textbox "Find"', "zzz")
	settle(app)
	expect(tally(app)).to_be("No results")
	app:key("Escape")
	settle(app)
	expect(app:has("#chat-find")).to_be(false)
	expect(app:error()).to_be(nil)
end)

test("the newest match shows where it is, not at its reply's start", function()
	local app = long_chat()
	app:command("find_open")
	settle(app)
	-- Typed a letter at a time, as a person does: each search starts at the
	-- newest match, which is in Part 100's code.
	for _, q in { "f", "fr", "fra", "fram", "frame" } do
		app:type('role:textbox "Find"', string.sub(q, -1))
		settle(app)
	end
	expect(tally(app)).to_be("602 of 602")
	local first = app:tree("#transcript"):match("[^\n]+") or ""
	local top, h = first:match(" %-?%d+,(%-?%d+) %d+x(%d+)")
	local y = y_of(app, "Part 100 explains")
	expect(y ~= nil).to_be(true)
	expect((y :: number) >= (tonumber(top) :: number) - 400 and (y :: number) < (tonumber(top) :: number) + (tonumber(h) :: number)).to_be(true)
	expect(y_of(app, "Part 1 explains")).to_be(nil)
	expect(app:error()).to_be(nil)
end)
