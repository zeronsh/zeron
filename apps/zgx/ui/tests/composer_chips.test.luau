-- Mentions picked in the composer are chips (ds/composer_mentions,
-- ds/composer_atoms): the name in the draft, what it stands for in the
-- chip's data. One unit as it's edited, copied and undone; kept with each
-- chat's draft; and sent as the same words the engine got when the draft
-- held the token itself. On the chat page (ui/chat_fixture).

local BOX = 'role:textbox "Message"'
-- (As crates/proto file_mentions.rs `local_file_link` writes a file, and
-- the fixture's agent lists its `/review`.)
local CHAT_RS = "[chat.rs](zeron-file:src/chat.rs)"
local REVIEW = "[/review](zeron-invoke:7b226b696e64223a22636f6d6d616e64222c226e616d65223a22726576696577227d)"

local function settle(app: any)
	app:frames(30)
end

local function chat(id: string?): any
	local app = mount("chat_fixture.luau")
	app:resize(900, 700)
	app:command(`go_{id or "s2"}`)
	settle(app)
	return app
end

-- `words` typed, then the list's first row for `query` (typed after its
-- trigger) taken.
local function pick(app: any, words: string, query: string, row: string)
	app:type(BOX, words .. query)
	app:wait("#never-shown", { timeout_ms = 300 })
	settle(app)
	expect(app:wait(`role:option ~"{row}"`, { timeout_ms = 2000 })).to_be(true)
	app:key("Enter")
	settle(app)
end

local function atoms(app: any): { any }
	return app:state("chat.composer.atoms")
end

local function sent(app: any, chat: string): string
	app:key("Enter")
	settle(app)
	app:wait("#never-shown", { timeout_ms = 300 })
	local calls: string = app:state("calls")
	return string.match(calls, `send:{chat}:(.*)$`) or ""
end

test("chips: a file and a command picked are chips, each carrying what it stands for, and go as the engine's words", function()
	local app = chat()
	pick(app, "Read ", "@chat", "chat.rs")
	pick(app, "then run ", "/rev", "/review")
	expect(app:text(BOX)).to_be("Read chat.rs then run /review ")
	local list = atoms(app)
	expect(#list).to_be(2)
	expect(list[1].data.kind).to_be("file")
	expect(list[1].data.value).to_be("src/chat.rs")
	expect(list[1].data.link).to_be(CHAT_RS)
	-- (Read aloud by its path.)
	expect(list[1].name).to_be("src/chat.rs")
	expect(list[2].data.kind).to_be("command")
	expect(list[2].data.link).to_be(REVIEW)
	-- What the engine gets: each chip as its link, the words around it as
	-- typed (the trailing space too), as when the draft held the tokens.
	expect(sent(app, "s2")).to_be(`Read {CHAT_RS} then run {REVIEW} `)
	expect(#atoms(app)).to_be(0)
end)

test("chips: Backspace takes a chip whole, and one undo puts it back whole", function()
	local app = chat()
	pick(app, "Look at ", "@chat", "chat.rs")
	expect(app:text(BOX)).to_be("Look at chat.rs ")
	-- The space after it, then the chip in one.
	app:key("Backspace")
	app:key("Backspace")
	settle(app)
	expect(app:text(BOX)).to_be("Look at ")
	expect(#atoms(app)).to_be(0)
	app:key("cmd+z")
	settle(app)
	expect(app:text(BOX)).to_be("Look at chat.rs")
	local back = atoms(app)
	expect(#back).to_be(1)
	expect(back[1].data.value).to_be("src/chat.rs")
	-- And it still goes as the file.
	expect(sent(app, "s2")).to_be(`Look at {CHAT_RS}`)
end)

test("chips: a chip copied from one chat's draft and pasted into another's keeps its path", function()
	local app = chat()
	pick(app, "Look at ", "@chat", "chat.rs")
	app:command("select-all")
	app:command("copy")
	app:command("go_s1")
	settle(app)
	app:type(BOX, "")
	app:command("paste")
	settle(app)
	expect(app:text(BOX)).to_be("Look at chat.rs ")
	local pasted = atoms(app)
	expect(#pasted).to_be(1)
	expect(pasted[1].data.value).to_be("src/chat.rs")
	expect(sent(app, "s1")).to_be(`Look at {CHAT_RS} `)
end)

test("chips: a chat's draft keeps its chips as another chat shows and it comes back", function()
	local app = chat()
	pick(app, "Look at ", "@chat", "chat.rs")
	app:command("go_s1")
	settle(app)
	expect(app:text(BOX)).to_be("")
	expect(#atoms(app)).to_be(0)
	app:command("go_s2")
	settle(app)
	expect(app:text(BOX)).to_be("Look at chat.rs ")
	local kept = atoms(app)
	expect(#kept).to_be(1)
	expect(kept[1].data.value).to_be("src/chat.rs")
	expect(sent(app, "s2")).to_be(`Look at {CHAT_RS} `)
end)

test("chips: a draft kept from before chips (its words alone) still comes back", function()
	local app = chat()
	app:set_state("chat.composer.drafts", { s1 = { text = "Look at @src/chat.rs closely", cargo = {} } })
	app:command("go_s1")
	settle(app)
	expect(app:text(BOX)).to_be("Look at @src/chat.rs closely")
	expect(#atoms(app)).to_be(0)
	expect(sent(app, "s1")).to_be("Look at @src/chat.rs closely")
end)

test("markdown: the caret never rests in a list item's folded marker", function()
	local app = chat()
	app:type(BOX, "intro\n- one")
	settle(app)
	local item = #"intro\n"
	-- The line's start (where its marker is folded to nothing): the caret
	-- goes on to the item's text, so what's typed there stays in the item.
	app:key("cmd+ArrowLeft")
	settle(app)
	local a, b = app:selection(BOX)
	expect(a).to_be(item + 2)
	expect(b).to_be(item + 2)
	-- Stepped back from the item's text: to the line before's end.
	app:key("ArrowLeft")
	settle(app)
	a, b = app:selection(BOX)
	expect(a).to_be(item - 1)
	expect(b).to_be(item - 1)
	-- And on, into the item's text again, past its marker.
	app:key("ArrowRight")
	settle(app)
	a = app:selection(BOX)
	expect(a).to_be(item + 2)
	-- The draft as typed: nothing rewritten.
	expect(app:text(BOX)).to_be("intro\n- one")
end)
