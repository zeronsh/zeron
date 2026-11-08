-- The chat page follows its session in place (ui/chat_fixture.luau, the
-- engine stubbed): a reply streaming grows its own row and builds no other
-- again; its tools arrive on the same row; scrolled up, the view holds
-- while it streams; the one composer keeps its draft and the keys across a
-- move from a new chat to a session and back.

-- A node's identity in the tree, by a pattern on its line: `@id`.
local function node(app: any, pattern: string): string?
	for line in app:tree():gmatch("[^\n]+") do
		if line:find(pattern, 1, true) then
			return line:match("@(%d+)")
		end
	end
	return nil
end

-- The last node matching: the live reply is the transcript's last.
local function last(app: any, pattern: string): string?
	local found = nil
	for line in app:tree():gmatch("[^\n]+") do
		if line:find(pattern, 1, true) then
			found = line:match("@(%d+)")
		end
	end
	return found
end

local function settle(app: any)
	app:frames(30)
end

local function open(app: any, id: string)
	app:command(`go_{id}`)
	settle(app)
end

test("a session's transcript shows from its end", function()
	local app = mount("chat_fixture.luau")
	app:resize(900, 700)
	open(app, "s1")
	expect(app:state("calls")).to_contain("open:s1")
	expect(app:wait('~"Question number 30"', { timeout_ms = 2000 })).to_be(true)
	expect(app:has('~"Question number 1:"')).to_be(false)
end)

test("a streaming reply grows in place, earlier rows untouched", function()
	local app = mount("chat_fixture.luau")
	app:resize(900, 700)
	open(app, "s1")
	app:command("start_reply")
	settle(app)
	local prompt = node(app, "Question number 30")
	local earlier = node(app, "Part 30 works like this.")
	expect(prompt).never.to_be_nil()
	expect(earlier).never.to_be_nil()
	app:command("stream_words")
	settle(app)
	expect(app:has('~"Here is a longer"')).to_be(true)
	local reply = last(app, 'article "Reply"')
	app:command("stream_words")
	app:command("stream_words")
	settle(app)
	expect(app:has('~"written a few"')).to_be(true)
	expect(`prompt @{node(app, "Question number 30")}`).to_be(`prompt @{prompt}`)
	expect(`earlier @{node(app, "Part 30 works like this.")}`).to_be(`earlier @{earlier}`)
	-- A tool arriving lands on the same reply.
	app:command("add_tool")
	settle(app)
	expect(app:has('~"1 command"')).to_be(true)
	expect(`reply @{last(app, 'article "Reply"')}`).to_be(`reply @{reply}`)
	-- Done: the same row, holding all it wrote.
	app:command("end_reply")
	settle(app)
	expect(`prompt @{node(app, "Question number 30")}`).to_be(`prompt @{prompt}`)
	expect(`reply @{last(app, 'article "Reply"')}`).to_be(`reply @{reply}`)
	expect(app:has('~"written a few"')).to_be(true)
end)

test("scrolled up, the view holds while a reply streams", function()
	local app = mount("chat_fixture.luau")
	app:resize(900, 700)
	open(app, "s1")
	app:scroll("#transcript", -900)
	settle(app)
	local at = app:state("chat.transcript.offset")
	expect(at < 1e8).to_be(true)
	app:command("start_reply")
	settle(app)
	for _ = 1, 6 do
		app:command("stream_words")
		app:frames(4)
	end
	settle(app)
	expect(math.abs(app:state("chat.transcript.offset") - at) < 1).to_be(true)
	expect(app:wait('role:button "Jump to latest"', { text = "New reply", timeout_ms = 2000 })).to_be(true)
end)

test("each chat keeps its own draft; the composer stays one node with the keys", function()
	local app = mount("chat_fixture.luau")
	app:resize(900, 700)
	settle(app)
	local field = node(app, "textbox")
	local function line(): string
		return app:tree():match("[^\n]*textbox[^\n]*") or ""
	end
	-- In s1.
	open(app, "s1")
	app:type('role:textbox "Message"', "Draft for one")
	settle(app)
	-- To s2: its own, empty; the same composer, the keys still its.
	open(app, "s2")
	expect(`field @{node(app, "textbox")}`).to_be(`field @{field}`)
	expect(app:text('role:textbox "Message"')).to_be("")
	expect(line()).to_contain("focused")
	app:type('role:textbox "Message"', "Draft for two")
	settle(app)
	-- Back to s1: its draft again.
	open(app, "s1")
	expect(app:text('role:textbox "Message"')).to_be("Draft for one")
	-- A new chat: its own, empty; typed in, kept as its own.
	app:command("go_home")
	settle(app)
	expect(`field @{node(app, "textbox")}`).to_be(`field @{field}`)
	expect(app:text('role:textbox "Message"')).to_be("")
	app:type('role:textbox "Message"', "A new idea")
	settle(app)
	open(app, "s2")
	expect(app:text('role:textbox "Message"')).to_be("Draft for two")
	app:command("go_home")
	settle(app)
	expect(app:text('role:textbox "Message"')).to_be("A new idea")
	-- Sending clears only that chat's: s1's stays.
	open(app, "s2")
	app:key("Enter")
	app:wait('~"Draft for two"', { timeout_ms = 1000 })
	expect(app:text('role:textbox "Message"')).to_be("")
	open(app, "s1")
	expect(app:text('role:textbox "Message"')).to_be("Draft for one")
end)

-- The first message's words at the view's top, and where.
local function top(app: any): string
	for line in app:tree("#transcript"):gmatch("[^\n]+") do
		local y = tonumber(line:match(" %-?%d+,(%-?%d+) "))
		if line:find('text "', 1, true) and y and y >= 38 then
			return `{line:match('text "([^"]*)"')} @{y}`
		end
	end
	return "?"
end

test("a session page made again keeps its place, mid-reply", function()
	local app = mount("chat_fixture.luau")
	app:resize(900, 700)
	open(app, "s1")
	app:command("start_reply")
	app:command("stream_words")
	settle(app)
	app:scroll("#transcript", -900)
	settle(app)
	-- (Where it is is written once a scroll settles: let it.)
	app:wait("#never-shown", { timeout_ms = 400 })
	local was = top(app)
	-- The page made again, as a reload or a saved template makes it: the
	-- same words at the top, the reply still being written below.
	app:command("remake_session")
	settle(app)
	app:wait("#never-shown", { timeout_ms = 200 })
	expect(top(app)).to_be(was)
	app:command("stream_words")
	settle(app)
	expect(top(app)).to_be(was)
	-- Down at the reply: it kept every word written, across the remake.
	app:scroll("#transcript", 5000)
	settle(app)
	expect(app:has('~"Here is a longer answer, written"')).to_be(true)
end)

test("at the end, a page made again stays at the end", function()
	local app = mount("chat_fixture.luau")
	app:resize(900, 700)
	open(app, "s1")
	app:command("remake_session")
	settle(app)
	expect(app:wait('~"Question number 30"', { timeout_ms = 2000 })).to_be(true)
end)

-- The way back to the latest, wherever the page was made: in a page made
-- again mid-reply (a reload), and after a send. (In a window whose page had
-- been made again, the way back once never showed: a transcript made before
-- wrote its count over the shown one's.)
test("scrolled up in a page made again, the way back to the latest shows", function()
	local app = mount("chat_fixture.luau")
	app:resize(900, 700)
	open(app, "s1")
	app:command("start_reply")
	app:command("stream_words")
	settle(app)
	app:command("remake_session")
	settle(app)
	app:command("remake_session")
	settle(app)
	app:scroll("#transcript", -900)
	settle(app)
	expect(app:wait('role:button "Jump to latest"', { text = "New reply", timeout_ms = 2000 })).to_be(true)
	settle(app)
	app:click('role:button "Jump to latest"')
	expect(app:wait('role:button "Jump to latest"', { gone = true, timeout_ms = 2000 })).to_be(true)
end)

test("scrolled up after a send, the way back to the latest shows", function()
	local app = mount("chat_fixture.luau")
	app:resize(900, 700)
	open(app, "s1")
	app:type('role:textbox "Message"', "One more thing")
	app:key("Enter")
	settle(app)
	-- (The prompt glides to the top: let it land.)
	app:wait("#never-shown", { timeout_ms = 700 })
	expect(app:has('~"One more thing"')).to_be(true)
	app:command("start_reply")
	app:command("stream_words")
	settle(app)
	app:scroll("#transcript", -900)
	settle(app)
	expect(app:wait('role:button "Jump to latest"', { timeout_ms = 2000 })).to_be(true)
end)

test("the catalog changing keeps the composer, its draft and the keys", function()
	local app = mount("chat_fixture.luau")
	app:resize(900, 700)
	settle(app)
	local field = node(app, "textbox \"Message\"")
	app:type('role:textbox "Message"', "Halfway through a thought")
	app:command("catalog_grows")
	settle(app)
	expect(`field @{node(app, "textbox \"Message\"")}`).to_be(`field @{field}`)
	expect((app:tree():match("[^\n]*textbox \"Message\"[^\n]*")) or "").to_contain("focused")
	-- The new agent's model is there to pick.
	app:click('role:button "Model"')
	settle(app)
	expect(app:has('role:tab "Codex"')).to_be(true)
end)

test("a catalog with no agents shows the picker without failing", function()
	local app = mount("chat_fixture.luau")
	app:resize(900, 700)
	app:command("catalog_empty")
	app:set_state("chat_model", nil)
	settle(app)
	app:click('role:button "Model"')
	settle(app)
	expect(app:error()).to_be_nil()
	app:command("catalog_grows")
	settle(app)
	expect(app:error()).to_be_nil()
	expect(app:has('role:tab "Codex"')).to_be(true)
end)

-- A node's box, by a pattern on its line: x, y, w, h.
local function box(app: any, pattern: string, last_one: boolean?): (number, number, number, number)
	local found: string? = nil
	for line in app:tree():gmatch("[^\n]+") do
		if line:find(pattern, 1, true) then
			found = line
			if not last_one then
				break
			end
		end
	end
	assert(found, `nothing like {pattern}`)
	local x, y, w, h = (found :: string):match(" (%-?%d+),(%-?%d+) (%d+)x(%d+)")
	return tonumber(x) :: number, tonumber(y) :: number, tonumber(w) :: number, tonumber(h) :: number
end

test("the conversation runs under the composer, its end held clear of it", function()
	local app = mount("chat_fixture.luau")
	app:resize(900, 700)
	open(app, "s1")
	local _, list_y, _, list_h = box(app, "#transcript")
	local _, dock_y, _, dock_h = box(app, "#chat-composer")
	-- The scroll view runs on behind the composer, to its footer's line.
	expect(list_y + list_h > dock_y + 40).to_be(true)
	expect(list_y + list_h <= dock_y + dock_h).to_be(true)
	-- At the end: the last message's words sit above the composer.
	local _, text_y, _, text_h = box(app, 'text "It reads its input', true)
	expect(text_y + text_h <= dock_y).to_be(true)
	-- Scrolled up past the last reply's room: the conversation is laid
	-- out behind the composer.
	app:scroll("#transcript", -600)
	settle(app)
	local behind = false
	for line in app:tree("#transcript"):gmatch("[^\n]+") do
		local y, h = line:match('article "%a+" %-?%d+,(%-?%d+) %d+x(%d+)')
		local top, tall = tonumber(y), tonumber(h)
		if top and tall and top < dock_y + dock_h and top + tall > dock_y then
			behind = true
		end
	end
	expect(behind).to_be(true)
	-- And the way back stands over the composer, not under it.
	expect(app:wait('role:button "Jump to latest"', { timeout_ms = 2000 })).to_be(true)
	app:wait("#never-shown", { timeout_ms = 300 })
	local _, pill_y, _, pill_h = box(app, 'button "Jump to latest"')
	expect(pill_y + pill_h <= dock_y).to_be(true)
end)

-- The y of the first line holding `words`, or nil.
local function _y_of(app: any, words: string): number?
	for line in app:tree("#transcript"):gmatch("[^\n]+") do
		if line:find(words, 1, true) then
			local y = line:match(" %-?%d+,(%-?%d+) ")
			return tonumber(y)
		end
	end
	return nil
end

-- A send's glide, sampled: the prompt's y on each frame from the one it
-- joins in, whether the way back to the latest showed, and the prompt's id.
-- (A frame at a time: `app:wait` delivers as many frames as fit in its
-- wall-clock time, so a sample spanned three frames or five as the machine
-- was busy, and five crossed the bound below.)
local function glide(app: any, words: string): ({ number }, boolean, string?)
	app:type('role:textbox "Message"', words)
	app:key("Enter")
	local id: string? = nil
	local ys: { number } = {}
	local pill = false
	for _ = 1, 60 do
		app:frames(1)
		if not id then
			local ms = app:state("chat.messages")
			local last = ms[#ms]
			if last and last.text == words then
				id = last.id
			end
		end
		if id then
			-- The prompt's own node (the whole tree is too slow to read
			-- every frame).
			local first: string = app:tree("#" .. id):match("[^\n]+") or ""
			local y = tonumber(first:match(" %-?%d+,(%-?%d+) "))
			if y then
				table.insert(ys, y)
			end
		end
		pill = pill or app:has('role:button "Jump to latest"')
	end
	return ys, pill, id
end

-- Why a glide sampled so isn't one, or nil: it starts at the foot and lands
-- near the top, only ever up, no frame covering more than a fifth of the
-- way (a glide's most is under a tenth), through several frames between.
local function eased(ys: { number }): string?
	local from, to = ys[1], ys[#ys]
	if not from or from < 500 or to > 120 then
		return `from {from} to {to}: not from the foot to the top`
	end
	local between = 0
	for i = 2, #ys do
		local step = ys[i - 1] - ys[i]
		if step < -0.5 or step > 0.2 * (from - to) then
			return `frame {i}: {ys[i - 1]} to {ys[i]}`
		end
		if ys[i] < from - 2 and ys[i] > to + 2 then
			between += 1
		end
	end
	return if between >= 6 then nil else `only {between} frames between`
end

test("a sent prompt glides into its runway, and stays as its reply comes", function()
	local app = mount("chat_fixture.luau")
	app:resize(900, 700)
	open(app, "s1")
	-- From the foot (where the composer is) to near the top, gradually,
	-- and the way back to the latest never shows for a send's own glide.
	local ys, pill, id = glide(app, "Rise into place")
	expect(eased(ys)).to_be_nil()
	expect(pill).to_be(false)
	local to = ys[#ys]
	-- The engine's copy landing, the reply arriving and growing: it stays.
	local function at(): number?
		return tonumber((app:tree("#" .. (id :: string)):match("[^\n]+") or ""):match(" %-?%d+,(%-?%d+) "))
	end
	app:command("engine_copy")
	settle(app)
	expect(at()).to_be(to)
	app:command("start_reply")
	settle(app)
	expect(at()).to_be(to)
	app:command("stream_words")
	app:command("add_tool")
	settle(app)
	expect(at()).to_be(to)
	app:command("end_reply")
	settle(app)
	expect(at()).to_be(to)
end)

-- A node's y in the tree, by its id (its first line), or nil.
local function y_of_id(app: any, id: string): number?
	return tonumber((app:tree("#" .. id):match("[^\n]+") or ""):match(" %-?%d+,(%-?%d+) "))
end

-- A steer, as a person makes it mid-turn: the queued row's Steer, then the
-- engine's copy of it landing a few frames on. Sampled a frame at a time:
-- the steered prompt's y (`q1`) from the frame it joins in, and the lowest
-- `watch` (an earlier prompt, by its id) stood meanwhile.
local function steer(app: any, watch: string?): ({ number }, number?)
	app:click('role:button "Open Queued"')
	settle(app)
	app:hover('role:listitem "1. Queued words"')
	settle(app)
	local lowest: number? = if watch then y_of_id(app, watch) else nil
	local function look()
		local y = if watch then y_of_id(app, watch) else nil
		if y and (lowest == nil or y > (lowest :: number)) then
			lowest = y
		end
	end
	app:click('role:button "Steer"')
	for _ = 1, 4 do
		app:frames(1)
		look()
	end
	expect(app:state("calls")).to_contain("queue:s1:send:q1")
	app:command("steer_lands")
	local ys: { number } = {}
	for _ = 1, 60 do
		app:frames(1)
		local y = y_of_id(app, "q1")
		if y then
			table.insert(ys, y)
		end
		look()
	end
	return ys, lowest
end

test("a steered prompt glides into its runway, as a sent one does", function()
	local app = mount("chat_fixture.luau")
	app:resize(900, 700)
	open(app, "s1")
	-- The turn runs (the conversation opened as it stands: no runway yet),
	-- and a message waits in the queue.
	app:command("start_reply")
	app:command("stream_words")
	app:command("queue_one")
	settle(app)
	local ys = steer(app)
	expect(eased(ys)).to_be_nil()
	-- It stands where a sent prompt does: at the view's top, its reply's room
	-- under it, and stays there.
	local top = tonumber((app:tree("#transcript"):match("[^\n]+") or ""):match(" %-?%d+,(%-?%d+) ")) :: number
	local to = ys[#ys]
	expect(to - top < 60).to_be(true)
	settle(app)
	expect(y_of_id(app, "q1")).to_be(to)
end)

test("a steer keeps the runway a send made, and the steered prompt takes it on", function()
	local app = mount("chat_fixture.luau")
	app:resize(900, 700)
	open(app, "s1")
	local _, _, id = glide(app, "Rise into place")
	app:command("engine_copy")
	app:command("start_reply")
	app:command("stream_words")
	app:command("queue_one")
	settle(app)
	local sent = y_of_id(app, id :: string) :: number
	-- The prompt sent stands at the top with its reply under it.
	local ys, lowest = steer(app, id)
	-- The view never falls back as the steer lands (the runway dropped, the
	-- end coming down to the content's): the prompt sent only ever rises,
	-- but for the glide's one-point step clear of the end, as a send's.
	expect((lowest :: number) - sent <= 1).to_be(true)
	expect(eased(ys)).to_be_nil()
	-- The steered prompt lands where the sent one stood.
	expect(math.abs(ys[#ys] - sent) <= 2).to_be(true)
	settle(app)
	expect(math.abs((y_of_id(app, "q1") :: number) - sent) <= 2).to_be(true)
end)

-- Each "You" article's height, top to bottom.
local function bubbles(app: any): { number }
	local out = {}
	for line in app:tree("#transcript"):gmatch("[^\n]+") do
		local h = line:match('article "You" %-?%d+,%-?%d+ %d+x(%d+)')
		if h then
			table.insert(out, tonumber(h) :: number)
		end
	end
	return out
end

test("long messages open folded, never at their whole height first; a click eases them", function()
	local app = mount("chat_fixture.luau")
	app:resize(900, 900)
	app:command("go_s4")
	-- From the first frame: every long bubble at the fold's height.
	local folded: number? = nil
	for frame = 1, 40 do
		app:frames(1)
		for _, h in bubbles(app) do
			folded = folded or h
			if h ~= folded then
				error(`frame {frame}: a long message stood {h} tall, not the fold's {folded}`)
			end
		end
	end
	expect(folded ~= nil).to_be(true)
	expect((folded :: number) < 260).to_be(true)
	-- Show more: it eases open (heights between the two on the way).
	-- (The last one shown: the one measured below.)
	local n = app:count('role:button "Expand message"')
	app:click(`role:button "Expand message"[{n}]`)
	local seen: { number } = {}
	for _ = 1, 30 do
		app:wait("#never-shown", { timeout_ms = 16 })
		local hs = bubbles(app)
		table.insert(seen, hs[#hs])
	end
	local full = seen[#seen]
	expect(full > (folded :: number) + 100).to_be(true)
	local between = false
	for _, h in seen do
		if h > (folded :: number) + 2 and h < full - 2 then
			between = true
		end
	end
	expect(between).to_be(true)
	-- Opened stays opened: the session left and come back to.
	app:command("go_s1")
	settle(app)
	app:command("go_s4")
	settle(app)
	local again = bubbles(app)
	expect(again[#again]).to_be(full)
end)

test("every send glides: the first after opening, and the next after a glide lands", function()
	local app = mount("chat_fixture.luau")
	app:resize(900, 700)
	open(app, "s1")
	local first, pill1 = glide(app, "The first one")
	expect(eased(first)).to_be_nil()
	expect(pill1).to_be(false)
	app:command("engine_copy")
	app:command("start_reply")
	app:command("stream_words")
	app:command("end_reply")
	settle(app)
	local second, pill2 = glide(app, "And the next one")
	expect(eased(second)).to_be_nil()
	expect(pill2).to_be(false)
end)

test("a reply's tools group between its text, and a tool joins the open group in place", function()
	local app = mount("chat_fixture.luau")
	app:resize(900, 900)
	open(app, "s5")
	-- In order: text, the first group (folded, done), text, the open group.
	local order = {}
	for line in app:tree("#transcript"):gmatch("[^\n]+") do
		local what = line:match('text "(First I.ll)') or line:match('button "(Ran 2 commands)')
			or line:match('text "(The second check)') or line:match('button "(Read 2 files)')
		if what then
			table.insert(order, what)
		end
	end
	expect(table.concat(order, " | ")).to_be("First I'll | Ran 2 commands | The second check | Read 2 files")
	-- Open at the tail: its tools show; the first group's don't.
	expect(app:has('~"main.rs"')).to_be(true)
	expect(app:has('~"git status"')).to_be(false)
	local first = node(app, 'button "Ran 2 commands')
	local words = node(app, "The second check")
	local open_group = node(app, 'button "Read 2 files')
	-- A third tool streams into the open group: it joins in place, the
	-- rest stays the same nodes.
	app:command("group_grows")
	settle(app)
	expect(app:has('~"fold"')).to_be(true)
	expect(`first @{node(app, 'button "Ran 2 commands')}`).to_be(`first @{first}`)
	expect(`words @{node(app, "The second check")}`).to_be(`words @{words}`)
	expect(`group @{node(app, 'button "Read 2 files')}`).to_be(`group @{open_group}`)
end)

-- A node's y, by its line's start in the tree.
local function y_in(tree: string, pattern: string): number?
	for line in tree:gmatch("[^\n]+") do
		if line:find(pattern, 1, true) then
			return tonumber(line:match(" %-?%d+,(%-?%d+) "))
		end
	end
	return nil
end

-- Until `ok` holds, looking every 50 ms up to `ms` (15 s): a full run
-- under load takes its time, the condition doesn't change.
local function eventually(app: any, ok: () -> boolean, ms: number?): boolean
	local tries = math.ceil((ms or 15000) / 50)
	for _ = 1, tries do
		if ok() then
			return true
		end
		app:wait('"never shown, only waited past"', { timeout_ms = 50 })
	end
	return ok()
end

test("a send says it's sending at once, then working, then the reply takes over", function()
	local app = mount("chat_fixture.luau")
	app:resize(900, 700)
	open(app, "s1")
	app:type('role:textbox "Message"', "Say something")
	app:key("Enter")
	-- The frame the prompt is there, "Sending…" is under it, no clock.
	local prompt: number?, line: number? = nil, nil
	expect(eventually(app, function(): boolean
		local t = app:tree("#transcript")
		prompt, line = y_in(t, '"Say something"'), y_in(t, '"Sending…"')
		return prompt ~= nil
	end)).to_be(true)
	expect(line ~= nil).to_be(true)
	expect((line :: number) > (prompt :: number)).to_be(true)
	-- The engine's copy lands, the turn not yet begun: still sending.
	app:command("engine_copy")
	settle(app)
	expect(app:has('~"Sending…"')).to_be(true)
	-- The turn begins: the working word and its clock, in the same place.
	expect(eventually(app, function(): boolean
		return y_in(app:tree("#transcript"), '"Say something"') ~= nil
	end)).to_be(true)
	app:wait('"never shown, only waited past"', { timeout_ms = 500 })
	local at = y_in(app:tree("#transcript"), '"Say something"')
	app:command("turn_starts")
	expect(eventually(app, function(): boolean
		return not app:has('~"Sending…"')
	end)).to_be(true)
	expect(eventually(app, function(): boolean
		return app:has('role:generic "Working"') or app:has('"Working"')
	end)).to_be(true)
	-- The reply begins: the prompt's line gives way to the reply's, the
	-- prompt where it was.
	app:command("start_reply")
	expect(eventually(app, function(): boolean
		return app:count('"Working"') == 1
	end)).to_be(true)
	expect(y_in(app:tree("#transcript"), '"Say something"')).to_be(at)
end)

test("a send that wasn't delivered says so, and sends again", function()
	local app = mount("chat_fixture.luau")
	app:resize(900, 700)
	open(app, "s1")
	app:command("undelivered")
	settle(app)
	expect(app:has('~"Not delivered"')).to_be(true)
	app:click('role:button "Not delivered — send again"')
	settle(app)
	expect(app:state("calls")).to_contain("retry:s1:lost1")
end)

test("no byline over a reply; its time and Copy show under it on hover, as Zeron's", function()
	local app = mount("chat_fixture.luau")
	app:resize(900, 700)
	open(app, "s6")
	app:scroll("#transcript", -3000)
	settle(app)
	-- No model or mark over the replies, no time at rest.
	expect(app:has('~"GPT-5.5"')).to_be(false)
	expect(app:has('~"Sep 8"')).to_be(false)
	expect(app:has('role:button "Copy message"')).to_be(false)
	-- The reply under the pointer shows its time and Copy, nothing moving.
	local tree = app:tree("#transcript")
	local paint = y_in(tree, '"Paint is second')
	app:hover('~"Paint is second"')
	settle(app)
	expect(app:has('"Sep 8, 1:43 PM"')).to_be(true)
	expect(app:count('role:button "Copy message"')).to_be(1)
	expect(y_in(app:tree("#transcript"), '"Paint is second')).to_be(paint)
	-- Under the reply's text, on its left.
	local stamp = y_in(app:tree("#transcript"), '"Sep 8, 1:43 PM"')
	expect(stamp ~= nil and paint ~= nil and stamp > paint).to_be(true)
	-- A prompt's, under its bubble.
	app:hover('~"Carry on from here"')
	settle(app)
	expect(app:has('"Sep 8, 1:42 PM"')).to_be(true)
	expect(app:has('"Sep 8, 1:43 PM"')).to_be(false)
end)

test("a move to another agent shows its seam over the prompt that moved it", function()
	local app = mount("chat_fixture.luau")
	app:resize(900, 700)
	open(app, "s6")
	app:scroll("#transcript", -3000)
	settle(app)
	expect(app:has('"Switched from Claude Code to Codex"')).to_be(true)
	local tree = app:tree("#transcript")
	local seam = y_in(tree, '"Claude Code"')
	local prompt = y_in(tree, '"Carry on from here')
	expect(seam ~= nil and prompt ~= nil and seam < prompt).to_be(true)
	-- Only once: the replies themselves carry no mark.
	expect(app:count('"Codex"')).to_be(1)
end)

test("a reply being written shows no time or Copy, even under the pointer", function()
	local app = mount("chat_fixture.luau")
	app:resize(900, 700)
	open(app, "s1")
	app:command("start_reply")
	app:command("stream_words")
	app:command("stream_words")
	settle(app)
	app:hover('~"Here is a longer"')
	settle(app)
	expect(app:has('"Sep 8, 5:00 PM"')).to_be(false)
	expect(app:has('role:button "Copy message"')).to_be(false)
end)

-- The time of the reply `id` ("a7" → "Sep 8, 4:17 PM").
local function time_of(id: string): string
	return `Sep 8, 4:{10 + (tonumber(string.sub(id, 2)) :: number)} PM`
end

test("the lane goes with the pointer as the list scrolls under it", function()
	local app = mount("chat_fixture.luau")
	app:resize(900, 700)
	open(app, "s1")
	app:hover('~"Part 30 works like this"')
	settle(app)
	expect(app:state("transcript_hover")).to_be("a30")
	expect(app:count('"' .. time_of("a30") .. '"')).to_be(1)
	-- The list scrolls up under the still pointer, far enough that the
	-- hovered row goes out of view.
	app:scroll("#transcript", -1500)
	app:wait("#never-shown", { timeout_ms = 120 })
	settle(app)
	local now = app:state("transcript_hover") :: string
	expect(now).never.to_be("a30")
	expect(app:has('~"' .. time_of("a30") .. '"')).to_be(false)
	-- At most one lane shows, and it is the message's under the pointer.
	local shown = 0
	for i = 1, 30 do
		if app:has('"' .. time_of(`a{i}`) .. '"') then
			shown += 1
			expect(`a{i}`).to_be(now)
		end
	end
	expect(shown <= 1).to_be(true)
end)

test("a chat opened as it stands shows its end, with no runway under its last turn", function()
	local app = mount("chat_fixture.luau")
	app:resize(960, 640)
	open(app, "s8")
	-- Zeron holds the view under a prompt only for a turn sent from this
	-- window: opened, the conversation fills the view to its end, its
	-- earlier turn in sight above the last.
	local tree = app:tree("#transcript")
	local first = y_in(tree, '"First I\'ll look around the tests."')
	local last = y_in(tree, '"Done. The release workflow is running."')
	expect(first ~= nil and first > 0).to_be(true)
	expect(last ~= nil and first ~= nil and last > (first :: number)).to_be(true)
	-- A send from here does hold it: the prompt rises to the top.
	app:type('role:textbox "Message"', "One more thing")
	app:key("Enter")
	app:wait("#never-shown", { timeout_ms = 700 })
	settle(app)
	local prompt = y_in(app:tree("#transcript"), '"One more thing"')
	expect(prompt ~= nil and (prompt :: number) < 120).to_be(true)
end)

-- Leaving sessions lets the chat go (the page's cleanup, a turn on, once
-- the route has left): a timer started as the page goes still runs.
test("leaving sessions lets the watched chat go", function()
	local app = mount("chat_fixture.luau")
	app:resize(900, 700)
	open(app, "s1")
	expect(app:state("calls")).to_contain("open:s1")
	app:command("go_home")
	settle(app)
	app:wait("#never-shown", { timeout_ms = 100 })
	settle(app)
	expect(app:state("calls")).to_contain("open:none")
end)

-- The names plugins place by (the contract while zgx learns to place by
-- class): the session page, the transcript, each message and its actions
-- by kind, the composer and its toolbar; a new chat its own page.
test("the chat page's parts carry the names plugins place by", function()
	local app = mount("chat_fixture.luau")
	app:resize(900, 700)
	open(app, "s6")
	expect(app:has("#chat-session")).to_be(true)
	expect(app:has("#transcript")).to_be(true)
	expect(app:has("#chat-composer")).to_be(true)
	expect(app:has("#composer-toolbar")).to_be(true)
	-- (`.message`, `.message-user`, `.message-assistant` and
	-- `.message-actions` are set too; zgx's test selectors don't match a
	-- class yet, so they're checked once it does.)
	app:command("go_home")
	settle(app)
	expect(app:has("#chat-home")).to_be(true)
	expect(app:has("#chat-session")).to_be(false)
end)

-- Settings' "Conversation width": the transcript's column and the composer
-- follow it as it moves.
test("the conversation width setting widens the transcript and the composer", function()
	local app = mount("chat_fixture.luau")
	app:resize(1400, 800)
	open(app, "s2")
	local function width(sel: string): number
		local line = string.match(app:tree(sel), "[^\n]+") or ""
		return tonumber(string.match(line, " %-?%d+,%-?%d+ (%d+)x%d+")) or 0
	end
	local function widest_reply(): number
		local w = 0
		for line in app:tree("#transcript"):gmatch("[^\n]+") do
			if line:find('article "Reply"', 1, true) then
				w = math.max(w, tonumber(string.match(line, " %-?%d+,%-?%d+ (%d+)x%d+")) or 0)
			end
		end
		return w
	end
	expect(width("#chat-composer")).to_be(660)
	-- (The transcript a little wider than the composer each side, to fade behind it.)
	expect(widest_reply() <= 660 + 24).to_be(true)
	app:set_state("settings_conversation_width", 900)
	app:wait("#never-shown", { timeout_ms = 900 })
	settle(app)
	expect(width("#chat-composer")).to_be(900)
	local w = widest_reply()
	expect(w > 660 + 24 and w <= 900 + 24).to_be(true)
end)
