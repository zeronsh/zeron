-- What the chat page asks of the host (ui/chat_fixture.luau's stubs note
-- each call in `calls`): sending to a session, starting a new chat and
-- landing on it, answering an approval, a model chosen reaching the
-- session, and Stop.

local function settle(app: any)
	app:frames(30)
end

local function open(app: any, id: string)
	app:command(`go_{id}`)
	settle(app)
end

test("a message typed in a session goes to it, and shows at once", function()
	local app = mount("chat_fixture.luau")
	app:resize(900, 700)
	open(app, "s2")
	app:type("role:textbox", "Ship it")
	app:key("Enter")
	settle(app)
	-- (It joins the transcript a frame on, as its glide starts, and goes to
	-- the host then.)
	expect(app:wait('~"Ship it"', { timeout_ms = 1000 })).to_be(true)
	expect(app:state("calls")).to_contain("send:s2:Ship it")
	expect(app:text("role:textbox")).to_be("")
end)

test("a new chat is made in the project, sent its first message, and opened", function()
	local app = mount("chat_fixture.luau")
	app:resize(900, 700)
	app:command("go_home")
	settle(app)
	app:type("role:textbox", "Start the release")
	app:key("Enter")
	settle(app)
	local calls = app:state("calls")
	expect(calls).to_contain("create:atlas:")
	expect(calls).to_contain("send:new1:Start the release")
	expect(calls).to_contain("open:new1")
	-- Opened before it's sent: the host shows the prompt at once only for
	-- the chat it has open, so the new page never starts empty.
	expect(calls:find("open:new1", 1, true) < calls:find("send:new1", 1, true)).to_be(true)
	expect(app:wait('~"Start the release"', { timeout_ms = 2000 })).to_be(true)
end)

test("an approval shows in the composer, allowed once", function()
	local app = mount("chat_fixture.luau")
	app:resize(900, 700)
	open(app, "s1")
	app:command("ask_approval")
	settle(app)
	expect(app:has('~"rm -rf build"')).to_be(true)
	-- The engine allows once: there's no for-good.
	expect(app:has('role:button "Always allow"')).to_be(false)
	app:click('role:button "Allow"')
	settle(app)
	expect(app:state("calls")).to_contain("approve:s1:r1:once")
	expect(app:wait('~"rm -rf build"', { gone = true, timeout_ms = 2000 })).to_be(true)
end)

-- The session as the page has it: its agent and model.
local function running_with(app: any): string
	local s = app:state("chat.session") or {}
	return `{s.harness or "-"}|{s.model or "-"}`
end

local function configured(app: any, id: string): boolean
	return string.find(app:state("calls") or "", `configure:{id}:`, 1, true) ~= nil
end

test("a model picked in a session waits for a message: nothing of the session changes", function()
	local app = mount("chat_fixture.luau")
	app:resize(900, 700)
	open(app, "s2")
	expect(app:state("chat_model")).to_be("sonnet")
	local before = running_with(app)
	-- Another agent's model, picked and not sent.
	app:set_state("chat_model", "gpt")
	settle(app)
	expect(app:state("chat_model")).to_be("gpt")
	-- No call, and the session (its agent, so its icon) as it was.
	expect(configured(app, "s2")).to_be(false)
	expect(running_with(app)).to_be(before)
end)

test("a pick goes with the message, and the session takes it then", function()
	local app = mount("chat_fixture.luau")
	app:resize(900, 700)
	-- (Codex's models join the catalog.)
	app:command("catalog_grows")
	settle(app)
	open(app, "s2")
	app:set_state("chat_model", "gpt")
	settle(app)
	app:type('role:textbox "Message"', "Try it with GPT")
	app:key("Enter")
	settle(app)
	expect(app:wait('~"Try it with GPT"', { timeout_ms = 1000 })).to_be(true)
	settle(app)
	local calls = app:state("calls")
	expect(calls).to_contain("send:s2:Try it with GPT")
	expect(calls).to_contain("took:s2:codex|gpt|")
	expect(running_with(app)).to_be("codex|gpt")
	-- Through the send, not a change of its own.
	expect(configured(app, "s2")).to_be(false)
	-- Taken: no pick left to carry.
	expect(app:state("chat.composer.picks").s2).to_be(nil)
end)

test("an unsent pick stays with its chat; picking the session's own again leaves none", function()
	local app = mount("chat_fixture.luau")
	app:resize(900, 700)
	open(app, "s2")
	app:set_state("chat_model", "gpt")
	settle(app)
	-- Another chat shows its own model, not this pick.
	open(app, "s1")
	expect(app:state("chat_model")).to_be("opus")
	expect(app:state("chat_effort")).to_be("high")
	-- Back: the pick, still unsent.
	open(app, "s2")
	expect(app:state("chat_model")).to_be("gpt")
	expect(configured(app, "s2")).to_be(false)
	-- The session's own model again: nothing kept.
	app:set_state("chat_model", "sonnet")
	settle(app)
	expect(app:state("chat.composer.picks").s2).to_be(nil)
	expect(configured(app, "s2")).to_be(false)
end)

test("Stop interrupts the running turn", function()
	local app = mount("chat_fixture.luau")
	app:resize(900, 700)
	open(app, "s1")
	app:command("start_reply")
	settle(app)
	app:click('role:button "Stop"')
	settle(app)
	expect(app:state("calls")).to_contain("interrupt:s1")
end)

test("words inserted from elsewhere land in the draft, with the keys", function()
	local app = mount("chat_fixture.luau")
	app:resize(900, 700)
	settle(app)
	app:type("role:textbox", "Look at")
	app:command("insert_words")
	settle(app)
	expect(app:text("role:textbox")).to_be("Look at\nsrc/chat.rs")
end)

test("@ lists the session's files as it's typed, and a pick lands in the draft", function()
	local app = mount("chat_fixture.luau")
	app:resize(900, 700)
	open(app, "s1")
	app:type('role:textbox "Message"', "Look at @chat")
	app:wait("#never-shown", { timeout_ms = 300 })
	settle(app)
	expect(app:state("calls")).to_contain("search:s1:chat")
	expect(app:wait('role:option ~"chat.rs"', { timeout_ms = 2000 })).to_be(true)
	expect(app:has('role:option ~"main.rs"')).to_be(false)
	app:key("Enter")
	settle(app)
	expect(app:text('role:textbox "Message"')).to_contain("chat.rs")
end)

test("an @ pick goes out as Zeron's file mention", function()
	local app = mount("chat_fixture.luau")
	app:resize(900, 700)
	open(app, "s2")
	app:type('role:textbox "Message"', "Look at @chat")
	app:wait("#never-shown", { timeout_ms = 300 })
	settle(app)
	expect(app:wait('role:option ~"chat.rs"', { timeout_ms = 2000 })).to_be(true)
	app:key("Enter")
	settle(app)
	expect(app:text('role:textbox "Message"')).to_contain("@chat.rs")
	app:key("Enter")
	settle(app)
	-- (It goes to the host as its glide starts.)
	expect(app:wait('~"Look at"', { timeout_ms = 1000 })).to_be(true)
	app:wait("#never-shown", { timeout_ms = 100 })
	-- (As crates/proto file_mentions.rs `local_file_link` writes it.)
	expect(app:state("calls")).to_contain("send:s2:Look at [chat.rs](zeron-file:src/chat.rs)")
end)

test("a file mention link escapes its name and encodes its path, as production's", function()
	local shared = require("../screens/chat/file_links")
	expect(shared.link("src/a file#[x].rs")).to_be("[a file#\\[x\\].rs](zeron-file:src/a%20file%23%5Bx%5D.rs)")
	expect(shared.link("src/components/", true)).to_be("[components](zeron-file:src/components/)")
	expect(shared.path_of("zeron-file:src/a%20file%23%5Bx%5D.rs")).to_be("src/a file#[x].rs")
	expect(shared.path_of("zeron-file:src/components/")).to_be("src/components/")
	expect(shared.path_of("zeron-file:../a.rs")).to_be(nil)
	expect(shared.path_of("zeron-file:/etc/passwd")).to_be(nil)
	expect(shared.path_of("https://example.com")).to_be(nil)
end)

test("a queued message edits in the composer and saves, the draft put back", function()
	local app = mount("chat_fixture.luau")
	app:resize(900, 700)
	open(app, "s1")
	app:type('role:textbox "Message"', "My own draft")
	app:command("queue_one")
	settle(app)
	-- (Its actions show under the pointer.)
	app:hover('role:listitem "1. Queued words"')
	settle(app)
	app:click('role:button "Edit"')
	settle(app)
	expect(app:state("calls")).to_contain("queue_edit:s1:q1")
	expect(app:text('role:textbox "Message"')).to_be("Queued words")
	app:key("Backspace")
	app:click('role:button "Save to queue"')
	settle(app)
	expect(app:state("calls")).to_contain("queue_finish:s1:q1:commit:Queued word")
	expect(app:text('role:textbox "Message"')).to_be("My own draft")
end)

test("opening a session never hands it the new chat's model", function()
	local app = mount("chat_fixture.luau")
	app:resize(900, 700)
	settle(app)
	-- A new chat's choice, then a session that has no model of its own.
	app:set_state("chat_model", "opus")
	settle(app)
	app:command("model_less")
	app:command("go_s3")
	settle(app)
	expect(app:state("calls")).never.to_contain("configure:s3")
	-- A choice made there is held for its next message (nothing of the
	-- session changes until one goes with it).
	app:set_state("chat_model", "sonnet")
	settle(app)
	expect(app:state("calls")).never.to_contain("configure:s3")
	expect(app:state("chat.composer.picks").s3.model).to_be("sonnet")
end)

-- Settings' composer and transcript preferences (screens/settings/prefs).

test("⌘↩ sends under \"mod_enter\", where ↩ breaks the line", function()
	local app = mount("chat_fixture.luau")
	app:resize(900, 700)
	open(app, "s2")
	app:set_state("settings_send_with", "mod_enter")
	settle(app)
	app:type("role:textbox", "Line one")
	app:key("Enter")
	settle(app)
	expect(app:state("calls")).never.to_contain("send:s2")
	expect(app:text("role:textbox")).to_contain("\n")
	app:key("cmd+Enter")
	-- (A send glides first, then reaches the host.)
	app:wait("#never-shown", { timeout_ms = 400 })
	settle(app)
	expect(app:state("calls")).to_contain("send:s2:Line one")
	-- Back to "enter": ↩ sends.
	app:set_state("settings_send_with", "enter")
	settle(app)
	app:type("role:textbox", "Two")
	app:key("Enter")
	app:wait("#never-shown", { timeout_ms = 400 })
	settle(app)
	expect(app:state("calls")).to_contain("send:s2:Two")
end)

test("Escape stops the running agent only with the setting on", function()
	local app = mount("chat_fixture.luau")
	app:resize(900, 700)
	open(app, "s1")
	app:command("start_reply")
	settle(app)
	app:type("role:textbox", "x")
	app:key("Backspace")
	app:key("Escape")
	settle(app)
	expect(app:state("calls")).never.to_contain("interrupt:s1")
	-- With the setting off, Escape gives the keyboard back instead (the
	-- window's bare letters work again, I to come back).
	expect(app:tree():find('textbox "[^"\n]*" focused')).to_be(nil)
	app:set_state("settings_escape_stops", true)
	settle(app)
	app:click("role:textbox")
	app:key("Escape")
	settle(app)
	expect(app:state("calls")).to_contain("interrupt:s1")
	app:set_state("settings_escape_stops", false)
end)

test("a compact transcript folds each reply's work into one group, its reply shown", function()
	local app = mount("chat_fixture.luau")
	app:resize(960, 640)
	open(app, "s8")
	expect(app:has('~"First I\'ll look around the tests."')).to_be(true)
	app:set_state("settings_compact_transcript", true)
	settle(app)
	expect(app:has('role:button "Worked for 12s"')).to_be(true)
	expect(app:has('~"First I\'ll look around the tests."')).to_be(false)
	expect(app:has('~"It passes now"')).to_be(true)
	-- Opened, its work shows: what it said, and its tools' group.
	app:click('role:button "Worked for 12s"')
	settle(app)
	expect(app:wait('~"First I\'ll look around the tests."', { timeout_ms = 1000 })).to_be(true)
	expect(app:has('role:button "Ran 1 command · read 1 file"')).to_be(true)
	app:set_state("settings_compact_transcript", false)
	settle(app)
	expect(app:has('role:button "Worked for 12s"')).to_be(false)
end)

-- The model chip's label rolls as the model changes: frame by frame, its
-- box eases once from the old name's width to the new's, and the roll ends
-- exactly where the settled text stands (no shrink, no shift on hand-back).
local function chip(app: any): (number?, { [string]: number })
	local tree = app:tree('role:button "Model"')
	local w: number? = nil
	local texts: { [string]: number } = {}
	for line in tree:gmatch("[^\n]+") do
		if not w then
			w = tonumber(line:match(" %-?%d+,%-?%d+ (%d+)x%d+"))
		end
		local s, x = line:match('text "([^"]*)" (%-?%d+),')
		if s and not line:find("hidden") then
			texts[s] = tonumber(x)
		end
	end
	return w, texts
end

test("a model's name rolls into place: the chip eases once, and nothing moves as the roll lets go", function()
	local app = mount("chat_fixture.luau")
	app:resize(900, 700)
	open(app, "s2")
	local before = chip(app)
	app:set_state("chat_model", "opus")
	local widths: { number } = {}
	local last_rolling: { [string]: number }? = nil
	local settled_at: number? = nil
	for f = 1, 90 do
		app:wait("#never-shown", { timeout_ms = 16 })
		local w, texts = chip(app)
		table.insert(widths, w or 0)
		if texts["O"] then
			last_rolling = texts
		elseif texts["Opus"] and last_rolling and not settled_at then
			settled_at = f
			-- Handed back: the settled name stands where its rolled letters
			-- ended.
			expect(texts["Opus"]).to_be(last_rolling["O"])
		end
	end
	expect(settled_at ~= nil).to_be(true)
	local final = widths[#widths]
	expect(final ~= before).to_be(true)
	-- From the frame it first reaches its final width, it stays there.
	local reached: number? = nil
	for i, w in widths do
		if not reached and math.abs(w - final) <= 1 then
			reached = i
		end
		if reached then
			expect(math.abs(w - final) <= 1).to_be(true)
		end
	end
	-- And it went there one way: never past it and back.
	local grows = final > (before :: number)
	for i = 2, #widths do
		if grows then
			expect(widths[i] >= widths[i - 1] - 1).to_be(true)
		else
			expect(widths[i] <= widths[i - 1] + 1).to_be(true)
		end
	end
end)

-- A passage attached from the conversation (the transcript's selection
-- Attach): one chip however often it's attached, sent after the words
-- with who said it and when, and shown on the sent message as a chip.

test("an attached passage goes with the prompt, who said it and when, and shows on the sent message", function()
	local app = mount("chat_fixture.luau")
	app:resize(960, 640)
	open(app, "s6")
	app:command("attach_passage")
	-- The same passage again (its menu and its bar both taken): still one.
	app:command("attach_passage")
	settle(app)
	expect(app:count('#chat-composer >> ~"GPT-5.5"')).to_be(1)
	app:type('role:textbox "Message"', "Why this one?")
	app:key("Enter")
	app:wait('"never shown, only waited past"', { timeout_ms = 600 })
	settle(app)
	local calls = app:state("calls")
	expect(calls).to_contain("send:s6:Why this one?\n\nSelected from this conversation:\n- GPT-5.5, Sep 8, 1:43 PM: Layout dominates on both machines")
	-- Sent: the words in the bubble, the passage a chip by it.
	expect(app:has('#transcript >> ~"Why this one?"')).to_be(true)
	expect(app:has('#transcript >> ~"Selected from this conversation"')).to_be(false)
	expect(app:has('#transcript >> ~"GPT-5.5, Sep 8, 1:43 PM"')).to_be(true)
	expect(app:has('#chat-composer >> ~"GPT-5.5"')).to_be(false)
end)

test("an attached passage's × takes it off, and nothing of it is sent", function()
	local app = mount("chat_fixture.luau")
	app:resize(960, 640)
	open(app, "s6")
	app:command("attach_passage")
	settle(app)
	app:hover('#chat-composer >> ~"GPT-5.5"')
	settle(app)
	app:click('role:button ~"Remove GPT-5.5"')
	settle(app)
	expect(app:has('#chat-composer >> ~"GPT-5.5"')).to_be(false)
	app:type('role:textbox "Message"', "Plain")
	app:key("Enter")
	app:wait('"never shown, only waited past"', { timeout_ms = 600 })
	settle(app)
	expect(app:state("calls")).to_contain("send:s6:Plain")
	expect(app:state("calls")).never.to_contain("Selected from this conversation")
end)

-- A tool's line holds to its row: a command of five lines (its newlines
-- left in, as the host once sent them) is one line in a row one line tall,
-- nothing of it below the row, the next tool right under it. Opened, the
-- whole command shows in its own block, all five lines, as tall as they are.

-- A node's box, from the tree: x, y, w, h of the first line matching.
local function box_of(app: any, sel: string, pattern: string): { x: number, y: number, w: number, h: number }?
	for line in app:tree(sel):gmatch("[^\n]+") do
		if line:find(pattern, 1, true) then
			local x, y, w, h = line:match(" (%-?%d+),(%-?%d+) (%d+)x(%d+)")
			if x then
				return { x = tonumber(x) :: number, y = tonumber(y) :: number, w = tonumber(w) :: number, h = tonumber(h) :: number }
			end
		end
	end
	return nil
end

test("a command of five lines is one line in its row, and opened shows them all", function()
	local app = mount("chat_fixture.luau")
	app:resize(960, 700)
	open(app, "s6")
	app:command("script_turn_raw")
	settle(app)
	local ran = box_of(app, "#transcript", "button \"Ran echo") :: any
	local words = box_of(app, "#transcript", "text \"echo '--- extension-ish") :: any
	local used = box_of(app, "#transcript", "button \"Used ToolSearch\"") :: any
	expect(ran ~= nil and words ~= nil and used ~= nil).to_be(true)
	expect(ran.h).to_be(28)
	-- Its words: one line, inside the row.
	expect(words.h < 24).to_be(true)
	expect(words.y >= ran.y and words.y + words.h <= ran.y + ran.h).to_be(true)
	-- The next tool, right under it.
	expect(used.y).to_be(ran.y + ran.h)

	-- The host's shape: the line one line, the whole command its call.
	app:command("script_turn")
	settle(app)
	local rows = {}
	for line in app:tree("#transcript"):gmatch("[^\n]+") do
		local id = line:match("^%s*(@%d+) button \"Ran echo")
		if id then
			table.insert(rows, id)
		end
	end
	expect(#rows).to_be(2)
	app:click(rows[2])
	settle(app)
	-- The block: the command's words taller than a line.
	local block = nil :: any
	for line in app:tree("#transcript"):gmatch("[^\n]+") do
		if line:find("text \"echo '--- extension-ish", 1, true) then
			local x, y, w, h = line:match(" (%-?%d+),(%-?%d+) (%d+)x(%d+)")
			if h and (tonumber(h) or 0) > 28 then
				block = { id = line:match("(@%d+)"), x = tonumber(x), y = tonumber(y), w = tonumber(w), h = tonumber(h) }
			end
		end
	end
	expect(block ~= nil).to_be(true)
	-- Five lines of the call's line height (xs: 14), and nothing cut.
	local full = app:text(block.id)
	expect(string.find(full, "--- issues", 1, true) ~= nil).to_be(true)
	expect(block.h >= 5 * 13 and block.h <= 5 * 18).to_be(true)
	-- The rows under it moved down by it, not under it.
	local ran2 = box_of(app, "#transcript", rows[2]) :: any
	local after = nil :: any
	local seen = false
	for line in app:tree("#transcript"):gmatch("[^\n]+") do
		if line:find(rows[2], 1, true) then
			seen = true
		elseif seen and line:find("button \"Used ToolSearch\"", 1, true) then
			local _, y = line:match(" (%-?%d+),(%-?%d+) ")
			after = tonumber(y)
			break
		end
	end
	expect(after ~= nil and after >= block.y + block.h).to_be(true)
	expect(ran2.h).to_be(28)
	-- Closed again: the block goes.
	app:click(rows[2])
	settle(app)
	local tall = false
	for line in app:tree("#transcript"):gmatch("[^\n]+") do
		local h = line:match("text \"echo '%-%-%- extension%-ish.* %-?%d+,%-?%d+ %d+x(%d+)")
		if h and (tonumber(h) or 0) > 28 then
			tall = true
		end
	end
	expect(tall).to_be(false)
end)

-- The files pane's files, added to the chat (its "Add to chat", or a drag):
-- chips like files from outside, once each, sent by their path on the
-- session's computer rather than uploaded.
test("project files added to the chat are chips, sent where they are", function()
	local app = mount("chat_fixture.luau")
	app:resize(900, 700)
	open(app, "s2")
	app:command("add_to_chat")
	app:command("add_to_chat")
	settle(app)
	expect(app:count('#chat-composer >> ~"lib.rs"')).to_be(1)
	expect(app:has('#chat-composer >> ~"notes.md"')).to_be(true)
	app:type("role:textbox", "Look at these")
	app:key("Enter")
	settle(app)
	expect(app:wait('~"Look at these"', { timeout_ms = 1000 })).to_be(true)
	settle(app)
	local calls = app:state("calls")
	expect(calls).to_contain("att:/tmp/atlas/src/lib.rs|host")
	expect(calls).to_contain("att:/tmp/atlas/docs/notes.md|host")
end)

-- The model picker opens folded on the chosen model's reasoning, the list
-- (its tabs, its search) a press away; Escape folds it back, then closes
-- it (Zeron's compact picker, the only one).
local function searching(app: any): boolean
	for line in app:tree():gmatch("[^\n]+") do
		if line:find('textbox "Search', 1, true) and not line:find(" hidden", 1, true) then
			return true
		end
	end
	return false
end
local function listed(app: any): boolean
	for line in app:tree():gmatch("[^\n]+") do
		if line:find(" dialog ", 1, true) and not line:find(" hidden", 1, true) then
			return true
		end
	end
	return false
end

test("the model picker opens folded, the list a press away", function()
	local app = mount("chat_fixture.luau")
	app:resize(900, 700)
	open(app, "s2")
	app:click('role:button "Model"')
	settle(app)
	expect(listed(app)).to_be(true)
	expect(searching(app)).to_be(false)
	-- A press on the chosen model (the layer over its one row) opens it out.
	app:click(".folded")
	settle(app)
	expect(searching(app)).to_be(true)
	app:key("Escape")
	settle(app)
	expect(listed(app)).to_be(true)
	expect(searching(app)).to_be(false)
	app:key("Escape")
	settle(app)
	expect(listed(app)).to_be(false)
end)

-- A model's options (Devin Fusion's lead and sidekick) are rows under it in
-- the picker; a choice made there is the chat's pick (sent with the next
-- message, as the model and effort are).
test("a model's options are picked under it in the composer's picker", function()
	local app = mount("chat_fixture.luau")
	app:resize(900, 700)
	open(app, "s2")
	app:set_state("chat_model", "fusion")
	settle(app)
	app:click('role:button "Model"')
	settle(app)
	expect(app:has('role:button "Lead"')).to_be(true)
	app:click('role:button "Lead"')
	settle(app)
	app:click('role:option ~"GPT-6 Sol"')
	settle(app)
	expect(((app:state("chat_options") or {}) :: any).lead).to_be("sol")
	app:key("Escape")
	settle(app)
	app:type("role:textbox", "Go")
	app:key("Enter")
	settle(app)
	expect(app:wait('~"Go"', { timeout_ms = 1000 })).to_be(true)
	-- Sent: the pick went with it (the model's, as a pick of the chat).
	expect(app:state("calls")).to_contain("took:s2:claude-code|fusion|")
	expect(app:error()).to_be(nil)
end)

-- A picture from a project on this computer is a picture chip (its
-- thumbnail read from where it is); one from a project elsewhere, a file
-- chip (the picture isn't here to show).
test("a project's picture shows as one when the project is on this computer", function()
	local app = mount("chat_fixture.luau")
	app:resize(900, 700)
	open(app, "s2")
	app:command("add_picture_to_chat")
	app:command("add_far_picture_to_chat")
	settle(app)
	local here, far = nil :: string?, nil :: string?
	for line in app:tree("#chat-composer"):gmatch("[^\n]+") do
		if line:find('"zgx.png"', 1, true) and not here then
			here = line
		elseif line:find('"logo.png"', 1, true) and not far then
			far = line
		end
	end
	expect(here ~= nil and far ~= nil).to_be(true)
	-- The picture chip is the thumbnail's size (56 wide); the file chip a
	-- row with its name (wider).
	local w_here = tonumber(string.match(here :: string, " %-?%d+,%-?%d+ (%d+)x%d+"))
	local w_far = tonumber(string.match(far :: string, " %-?%d+,%-?%d+ (%d+)x%d+"))
	expect(w_here).to_be(56)
	expect((w_far or 0) > 56).to_be(true)
	expect(app:error()).to_be(nil)
end)

-- The shell's ⌘/ opens the composer's model picker (chat.open_model_picker),
-- saying it did.
test("the model picker opens from a shortcut", function()
	local app = mount("chat_fixture.luau")
	app:resize(900, 700)
	open(app, "s2")
	local function listed(): boolean
		for line in app:tree():gmatch("[^\n]+") do
			-- (Its panel: folded on the reasoning, it lists nothing yet.)
			if line:find(" dialog ", 1, true) and not line:find(" hidden", 1, true) then
				return true
			end
		end
		return false
	end
	expect(listed()).to_be(false)
	app:command("open_model_picker")
	settle(app)
	expect(app:state("calls")).to_contain("model_picker:true")
	expect(listed()).to_be(true)
end)

-- An agent's completion settings (Settings → Providers): "Use $ for skills"
-- off makes `$` ordinary text; "Separate / commands" off lists the skills
-- under `/` with the commands (Zeron's composer.rs `completion_trigger`).
test("an agent's completion settings decide what $ and / open", function()
	local app = mount("chat_fixture.luau")
	app:resize(900, 700)
	open(app, "s2")
	-- (The agent's commands and skills: the fixture's stubbed list, a
	-- command `/review` and a skill "Code Review".)
	local function popup(): boolean
		for line in app:tree():gmatch("[^\n]+") do
			if line:find("mention-popover", 1, true) and not line:find(" hidden", 1, true) then
				return true
			end
		end
		return false
	end
	local function lists(name: string): boolean
		for line in app:tree():gmatch("[^\n]+") do
			if line:find(`"{name}"`, 1, true) and line:find("option", 1, true) and not line:find(" hidden", 1, true) then
				return true
			end
		end
		return false
	end
	local function typed(text: string)
		app:set_state("chat.composer.text", "")
		settle(app)
		app:type('role:textbox "Message"', text)
		settle(app)
	end
	-- Both on (the default): `$` opens the skills; `/` the commands alone.
	typed("$")
	expect(popup()).to_be(true)
	expect(lists("Code Review")).to_be(true)
	typed("/")
	expect(lists("/review")).to_be(true)
	expect(lists("Code Review")).to_be(false)
	-- This agent's (Claude Code's) both off.
	app:set_state("settings_skill_completion", { ["claude-code"] = { dollar = false, separate = false } })
	settle(app)
	typed("$")
	expect(popup()).to_be(false)
	typed("/")
	expect(lists("/review")).to_be(true)
	expect(lists("Code Review")).to_be(true)
	-- Another agent's settings leave this one alone.
	app:set_state("settings_skill_completion", { codex = { dollar = false } })
	settle(app)
	typed("$")
	expect(popup()).to_be(true)
end)

-- A GIF or HEIC dropped is a file chip (zgx can't draw either yet), so
-- nothing fails drawing a thumbnail; a PNG is a picture chip.
test("a dropped GIF or HEIC is a file chip, with no script error", function()
	local app = mount("chat_fixture.luau")
	app:resize(900, 700)
	open(app, "s2")
	app:command("drop_pictures")
	settle(app)
	local function chip_w(name: string): number
		for line in app:tree("#chat-composer"):gmatch("[^\n]+") do
			if line:find(`"{name}"`, 1, true) then
				return tonumber(string.match(line, " %-?%d+,%-?%d+ (%d+)x%d+")) or 0
			end
		end
		return 0
	end
	-- A picture chip is the thumbnail's 56 wide; a file chip, a wider row.
	expect(chip_w("zgx.png")).to_be(56)
	expect(chip_w("spin.gif") > 56).to_be(true)
	expect(chip_w("photo.heic") > 56).to_be(true)
	expect(app:error()).to_be(nil)
end)

-- Review comments left on files (the files pane's): chips on the composer,
-- sent after the words as Zeron's review-comments block, each its file and
-- line; with no words, Zeron's own line for them.
test("review comments go with the message as Zeron's block", function()
	local app = mount("chat_fixture.luau")
	app:resize(900, 700)
	open(app, "s2")
	app:command("leave_comments")
	settle(app)
	expect(app:has('#chat-composer >> ~"app.rs:12"')).to_be(true)
	expect(app:has('#chat-composer >> ~"guide.md:3"')).to_be(true)
	app:click('role:textbox "Message"')
	app:key("Enter")
	settle(app)
	app:wait("#never-shown", { timeout_ms = 300 })
	settle(app)
	expect(app:state("calls")).to_contain(
		"send:s2:Address the review comments below.\n\nReview comments (each cites the workspace file and line it belongs to):\n- src/app.rs:12: Rename this.\n  It reads badly.\n- docs/guide.md:3: Typo here.\n  And here."
	)
	-- Sent: one chip for them on the message.
	expect(app:has('#transcript >> ~"2 comments"')).to_be(true)
end)

test("a chat mentioned (a session dropped from the sidebar) goes as production's chat link", function()
	local links = require("../screens/chat/chat_links")
	expect(links.link("Brief  Hello\nInteraction", "0fd51606-ab")).to_be("[Brief Hello Interaction](zeron-chat:0fd51606-ab)")
	expect(links.link("a [b] `c` \\d", "x1")).to_be("[a \\[b\\] \\`c\\` \\\\d](zeron-chat:x1)")
	expect(links.link("t", "../etc")).to_be(nil)
	local app = mount("chat_fixture.luau")
	app:resize(900, 700)
	open(app, "s2")
	app:command("mention_s1")
	app:command("mention_s1")
	settle(app)
	app:type('role:textbox "Message"', "Compare with")
	app:key("Enter")
	settle(app)
	app:wait("#never-shown", { timeout_ms = 100 })
	local calls = app:state("calls")
	expect(calls).to_contain("send:s2:Compare with [")
	expect(calls).to_contain("](zeron-chat:s1)")
	-- (Once, though mentioned twice.)
	local _, n = string.gsub(calls, "zeron%-chat:s1", "")
	expect(n).to_be(1)
end)

-- An existing chat's footer says where it works, live, and opens nothing:
-- the branch git has checked out in its folder (not the one the chat began
-- on), with a worktree's icon. Only a new chat picks where to work.
test("a chat's footer shows its own branch, fixed, not the new chat's picker", function()
	local app = mount("chat_fixture.luau")
	app:resize(900, 700)
	app:command("go_s2")
	app:frames(30)
	app:command("session_in_worktree")
	app:frames(30)
	expect(app:state("calls")).to_contain("refs:/tmp/atlas-wt/pr:here")
	expect(app:wait('~"zeron/readme-pr"', { timeout_ms = 2000 })).to_be(true)
	-- A worktree's glyph, its project's name before it.
	expect(app:has("#composer-place-lead >> .place-lead-worktree")).to_be(true)
	expect(app:text("#composer-place-project[1]")).to_be("Atlas")
	-- No picker: nothing called Branch to open.
	expect(app:has('role:button "Branch"')).to_be(false)
	-- A new chat still has it.
	app:command("go_home")
	app:frames(30)
	expect(app:has('role:button "Branch"')).to_be(true)
	expect(app:error()).to_be(nil)
end)
