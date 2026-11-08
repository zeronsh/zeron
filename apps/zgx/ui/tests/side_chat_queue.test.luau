-- A side chat's composer shows what the chat page's does of its session, from
-- its own: a message sent while it works waits in its queue (never in its
-- transcript), steers from there, edits in its composer; what it asks shows
-- and is answered; its todos show. On the page in ui/lanes_fixture.luau.

local BOX = ".side-chat-tab >> role:textbox"
local ROW = '.side-chat-tab >> role:listitem "1. Queued words"'

local function open(): any
	local app = mount("lanes_fixture.luau")
	app:resize(1200, 900)
	app:frames(30)
	app:click('#session-lanes >> role:button "Open side chat Looking at the tests"')
	app:frames(30)
	return app
end

local function calls(app: any): string
	return tostring(app:state("calls") or "")
end

test("a message sent while a side chat works is queued, not put in its transcript", function()
	local app = open()
	app:command("side_running")
	app:frames(5)
	app:type(BOX, "one more thing")
	app:key("Enter")
	app:frames(10)
	expect(calls(app)).to_contain("send:side-a:one more thing")
	-- (Waiting for the turn it runs in: what's picked isn't sent with it.)
	expect(calls(app)).to_contain("pick:side-a::::")
	expect(app:has('.side-chat-tab >> "one more thing"')).to_be(false)
	expect(app:text(BOX)).to_be("")
end)

test("idle, a side chat's message joins its transcript as ever", function()
	local app = open()
	app:type(BOX, "one more thing")
	app:key("Enter")
	app:frames(10)
	expect(app:has('.side-chat-tab >> "one more thing"')).to_be(true)
	expect(calls(app)).to_contain("pick:side-a:codex/gpt-6.1")
end)

test("a queued message that doesn't go comes back to its composer", function()
	local app = open()
	app:command("side_running")
	app:command("send_fails")
	app:frames(5)
	app:type(BOX, "one more thing")
	app:key("Enter")
	app:frames(30)
	expect(app:text(BOX)).to_be("one more thing")
end)

test("a side chat's queue shows, and a row steers mid-turn", function()
	local app = open()
	app:command("side_running")
	app:command("side_queue_one")
	app:frames(20)
	-- (The queue waits in the pill, folded, until opened.)
	app:click('.side-chat-tab >> role:button "Open Queued"')
	app:frames(20)
	app:hover(ROW)
	app:frames(10)
	app:click('.side-chat-tab >> role:button "Steer"')
	app:frames(10)
	expect(calls(app)).to_contain("queue:side-a:send:q1")
end)

-- A node's y in the tree, by its id (its first line), or nil.
local function y_of_id(app: any, id: string): number?
	return tonumber((app:tree("#" .. id):match("[^\n]+") or ""):match(" %-?%d+,(%-?%d+) "))
end

test("a side chat's steered prompt rises to its top, its runway under it, as a send's does", function()
	local app = open()
	-- A prompt sent in it first: it holds its runway.
	app:type(BOX, "first words")
	app:key("Enter")
	app:frames(60)
	local ms = app:state("chat.lane.tab1.messages")
	local sent = ms[#ms].id
	local top = y_of_id(app, sent) :: number
	app:command("side_running")
	app:command("side_queue_one")
	app:frames(20)
	app:click(BOX)
	app:key("cmd+enter")
	app:frames(4)
	expect(calls(app)).to_contain("queue:side-a:send:q1")
	app:command("side_steer_lands")
	local ys: { number } = {}
	local lowest = top
	for _ = 1, 60 do
		app:frames(1)
		local y = y_of_id(app, "q1")
		if y then
			table.insert(ys, y)
		end
		local was = y_of_id(app, sent)
		if was and was > lowest then
			lowest = was
		end
	end
	-- The prompt sent never falls back (its runway dropped; only the
	-- glide's one-point step clear of the end, as a send's); the steered
	-- one rises from the foot, a frame at a time, to where the sent one stood.
	expect(lowest - top <= 1).to_be(true)
	expect(#ys > 10).to_be(true)
	expect(ys[1] - ys[#ys] > 100).to_be(true)
	local between = 0
	for i = 2, #ys do
		if ys[i] < ys[1] - 2 and ys[i] > ys[#ys] + 2 then
			between += 1
		end
	end
	expect(between >= 6).to_be(true)
	expect(math.abs(ys[#ys] - top) <= 2).to_be(true)
end)

test("⌘↩ with an empty draft steers the newest queued message", function()
	local app = open()
	app:command("side_running")
	app:command("side_queue_one")
	app:frames(20)
	app:click(BOX)
	app:key("cmd+enter")
	app:frames(10)
	expect(calls(app)).to_contain("queue:side-a:send:q1")
end)

test("a queued row comes off a side chat's queue", function()
	local app = open()
	app:command("side_queue_one")
	app:frames(20)
	-- (The queue waits in the pill, folded, until opened.)
	app:click('.side-chat-tab >> role:button "Open Queued"')
	app:frames(20)
	app:hover(ROW)
	app:frames(10)
	app:click('.side-chat-tab >> role:button "Remove"')
	app:frames(10)
	expect(calls(app)).to_contain("queue:side-a:remove:q1")
end)

test("a queued message edits in a side chat's composer and saves, its draft put back", function()
	local app = open()
	app:type(BOX, "My own draft")
	app:command("side_queue_one")
	app:frames(20)
	-- (The queue waits in the pill, folded, until opened.)
	app:click('.side-chat-tab >> role:button "Open Queued"')
	app:frames(20)
	app:hover(ROW)
	app:frames(10)
	app:click('.side-chat-tab >> role:button "Edit"')
	app:frames(10)
	expect(calls(app)).to_contain("queue_edit:side-a:q1")
	expect(app:text(BOX)).to_be("Queued words")
	app:key("Backspace")
	app:click('.side-chat-tab >> role:button "Save to queue"')
	app:frames(10)
	expect(calls(app)).to_contain("queue_finish:side-a:q1:commit:Queued word")
	expect(app:text(BOX)).to_be("My own draft")
end)

test("a queued message's file shows as a chip in a side chat's composer, and saving keeps it", function()
	local app = open()
	app:command("side_queue_with_file")
	app:frames(20)
	app:click('.side-chat-tab >> role:button "Open Queued"')
	app:frames(20)
	app:hover(ROW)
	app:frames(10)
	app:click('.side-chat-tab >> role:button "Edit"')
	app:frames(10)
	expect(calls(app)).to_contain("queue_edit:side-a:q2")
	expect(app:has('.side-chat-tab >> role:button "Remove note.txt"')).to_be(true)
	app:click('.side-chat-tab >> role:button "Save to queue"')
	app:frames(10)
	expect(calls(app)).to_contain("queue_finish:side-a:q2:commit:Queued words:files=/host/uploads/ea958e1e-note.txt@host")
	expect(app:has('.side-chat-tab >> role:button "Remove note.txt"')).to_be(false)
end)

test("what a side chat asks shows on its composer and is answered there", function()
	local app = open()
	app:command("side_approval")
	app:frames(20)
	expect(app:has('.side-chat-tab >> "Run this command?"')).to_be(true)
	app:click('.side-chat-tab >> role:button "Allow"')
	app:frames(10)
	expect(calls(app)).to_contain("approve:side-a:r1:once")
end)

test("a side chat's question shows on its composer", function()
	local app = open()
	app:command("side_question")
	app:frames(20)
	expect(app:has('.side-chat-tab >> "Which branch?"')).to_be(true)
end)

test("a side chat's todos show", function()
	local app = open()
	app:command("side_todos")
	app:frames(20)
	expect(app:tree(".side-chat-tab")).to_contain("Read the tests")
end)
