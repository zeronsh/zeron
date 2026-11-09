-- What a prompt's words say of what it carries (ds/attachment_mentions, the
-- proto's attachment_mentions.rs): a picture by its number in the draft,
-- `[Image N](zeron-image:N)`, a file by its name and number,
-- `[name](zeron-attachment:N)`; the engine gives an agent the plain label.
-- The words in words alone first, then on the chat page (ui/chat_fixture):
-- an `@` mention goes as its link, a picture up as `Image N`, a mention of
-- what's no longer attached as its plain label; the sent message, and the
-- same after a relaunch, shows each as its chip.

local AM = require("@ds/attachment_mentions")

local NNBSP = "\u{202F}"
local ODD = {
	`Screenshot 2026-11-07 at 1.10.14{NNBSP}PM.png`,
	"notes (draft) [v2] \u{1F4DD}.md",
	"see (attachment: other).txt",
	"back`tick\\slash.txt",
	string.rep("a-very-long-name-", 8) .. "end.txt",
}

test("words: each odd name goes into its link and comes back the same", function()
	for i, name in ODD do
		local link = AM.link(i, name)
		local found = AM.mentions(`before {link} after`)
		expect(#found).to_be(1)
		expect(found[1].label).to_be(name)
		expect(found[1].index).to_be(i)
		expect(found[1].image).to_be(false)
		expect(AM.prompt(`x {link} y`)).to_be(`x {name} y`)
	end
	local picture = AM.mentions(`see {AM.link(3)}`)
	expect(picture[1].label).to_be("Image 3")
	expect(picture[1].image).to_be(true)
end)

test("words: only links exactly as written count: not escaped text, code, a picture's own, a bad number or label", function()
	for _, text in {
		"\\[Image 1](zeron-image:1)",
		"`[Image 1](zeron-image:1)`",
		"![Image 1](zeron-image:1)",
		"[Image 2](zeron-image:1)",
		"[Image 0](zeron-image:0)",
		"[a]b](zeron-attachment:1)",
		"[x](zeron-attachment:01)",
		"[x](zeron-attachment:1x)",
	} do
		expect(`{text}: {#AM.mentions(text)}`).to_be(`{text}: 0`)
	end
end)

test("words: what's no longer attached goes as its plain label; what is keeps its link", function()
	local text = `{AM.link(1)} and {AM.link(2, "a [b].md")} and {AM.link(3, "c.txt")}`
	expect(AM.demote_unattached(text, { 1, 3 })).to_be(`{AM.link(1)} and a [b].md and {AM.link(3, "c.txt")}`)
end)

test("words: an upload's picture number and a file's name, as the engine keeps them", function()
	expect(AM.image_index("/x/uploads/92a7aa9f-Image_12.png")).to_be(12)
	expect(AM.image_index("Image 3.jpeg")).to_be(3)
	expect(AM.image_index("92a7aa9f-Screenshot_1.png")).to_be_nil()
	local m = AM.mentions(AM.link(2, `Shot at 1.10{NNBSP}PM (1).png`))[1]
	expect(AM.names(m, "/u/1b2c3d4e-Shot_at_1.10_PM__1_.png")).to_be(true)
	expect(AM.names(m, "/u/1b2c3d4e-Other.png")).to_be(false)
	local pic = AM.mentions(AM.link(4))[1]
	expect(AM.names(pic, "/u/1b2c3d4e-Image_4.png")).to_be(true)
end)

local BOX = 'role:textbox "Message"'

local function settle(app: any)
	app:frames(30)
end

local function chat(): any
	local app = mount("chat_fixture.luau")
	app:resize(1000, 800)
	app:command("go_s2")
	settle(app)
	return app
end

-- The draft's `@` list in Attachments, its row by name, taken.
local function mention(app: any, query: string)
	app:type(BOX, `@{query}`)
	settle(app)
	app:key("Enter")
	settle(app)
end

local function sent(app: any): string
	local calls = app:state("calls")
	return string.match(calls, "send:s2:(.*)$") or ""
end

test("a picture with a screenshot's name, mentioned, goes as Image N, up as Image N.png, and its chip shows", function()
	local app = chat()
	app:command("attach_odd")
	settle(app)
	app:type(BOX, "what is ")
	mention(app, "Screenshot")
	app:key("Enter")
	settle(app)
	expect(app:wait('~"what is"', { timeout_ms = 2000 })).to_be(true)
	local calls = app:state("calls")
	expect(calls).to_contain("send:s2:what is [Image 1](zeron-image:1)")
	expect(calls).to_contain("attname:Image 1.png")
	-- The sent message: the chip, its label.
	local tree = app:tree("#transcript")
	expect(tree).to_contain("what is Image 1")
	expect(tree).never.to_contain("zeron-image")
end)

test("files with odd names go as their escaped links, each by its own number, and show as their chips", function()
	local app = chat()
	app:command("attach_odd")
	settle(app)
	mention(app, "notes")
	mention(app, "see")
	app:key("Enter")
	settle(app)
	expect(app:wait('~"notes (draft)"', { timeout_ms = 2000 })).to_be(true)
	local calls = app:state("calls")
	expect(calls).to_contain(AM.link(2, "notes (draft) [v2] \u{1F4DD}.md"))
	expect(calls).to_contain(AM.link(3, "see (attachment: other).txt"))
	expect(app:tree("#transcript")).never.to_contain("zeron-attachment")
end)

test("two files of one name are two numbers, each its own", function()
	local app = chat()
	app:command("attach_odd")
	settle(app)
	app:type(BOX, "@same")
	settle(app)
	app:key("ArrowDown")
	app:key("Enter")
	settle(app)
	app:key("Enter")
	settle(app)
	expect(app:wait('~"same.txt"', { timeout_ms = 2000 })).to_be(true)
	expect(app:state("calls")).to_contain(AM.link(6, "same.txt"))
end)

test("one taken off before it goes is its plain label; one typed by hand that names nothing is words", function()
	local app = chat()
	app:command("attach_odd")
	settle(app)
	mention(app, "Screenshot")
	app:type(BOX, " and [mine](zeron-attachment:9)")
	settle(app)
	app:hover('role:button "Screenshot 2026-11-07 at 1.10.14' .. NNBSP .. 'PM.png"')
	app:frames(10)
	app:click('role:button "Remove Screenshot 2026-11-07 at 1.10.14' .. NNBSP .. 'PM.png"')
	settle(app)
	app:type(BOX, "")
	app:key("End")
	app:key("Enter")
	settle(app)
	expect(app:wait('~"and mine"', { timeout_ms = 2000 })).to_be(true)
	local said = sent(app)
	-- (Its name alone, as an agent reads one; the made-up one, its words.)
	expect(said).to_contain("Image 1 ")
	expect(said).to_contain(" and mine")
	expect(said).never.to_contain("zeron-image")
	expect(said).never.to_contain("zeron-attachment")
end)

test("an Appshot mentioned goes as its picture's number", function()
	local app = chat()
	app:set_state("appshots.captured", { n = 1, shot = { id = "a1", app = "Safari", bundle = "com.apple.Safari", title = "Release notes", path = "assets/composer/shell.png", bytes = 1024 } })
	settle(app)
	app:type(BOX, "look ")
	-- (By its window's title, its name.)
	mention(app, "Release")
	app:key("Enter")
	settle(app)
	expect(app:wait('~"look"', { timeout_ms = 2000 })).to_be(true)
	expect(app:state("calls")).to_contain("send:s2:look [Image 1](zeron-image:1)")
end)

test("after a relaunch, read back with its uploads' names, the message shows its chips", function()
	local app = chat()
	app:command("mention_turn")
	settle(app)
	local tree = app:tree("#mu1")
	expect(tree).to_contain("Look at Image 1 and notes [v2].md")
	expect(tree).never.to_contain("zeron-")
end)

-- A file tab and a saved side chat's tab dragged from the panel into the
-- composer (ui/panel_context_fixture): each is what the draft carries, the
-- file numbered as anything attached; `@` offers both under Attachments,
-- and each goes as its mention: the file's link by its number, the chat's
-- Zeron chat mention. The sent message shows both as chips.
test("dragged in from the panel, a file and a side chat are mentioned as what's attached", function()
	local app = mount("panel_context_fixture.luau")
	app:resize(1400, 900)
	app:frames(30)
	app:click('#shell-sidebar >> role:button "Fix the build"')
	app:command("side_chats")
	for _, command in { "context.file", "context.side" } do
		app:command(command)
		app:frames(30)
	end
	local field = "#chat-composer >> role:textbox"
	app:drag('#panel-strip >> role:tab "notes.md"', field)
	app:frames(20)
	app:drag('#panel-strip >> role:tab "Look into the flake"', field)
	app:frames(20)
	app:command("context.snapshot")
	local cargo = app:state("context.snapshot").main
	expect(cargo[1].mention).to_be(1)
	-- In @'s Attachments (Tab to it: All, Files, Chats, Attachments).
	local function pick(query: string)
		app:type(field, `@{query}`)
		app:frames(20)
		for _ = 1, 3 do
			app:key("Tab")
		end
		app:frames(10)
		expect(app:tree("#mention-launcher")).to_contain(`option "{if query == "notes" then "notes.md" else "Look into the flake"}"`)
		app:key("Enter")
		app:frames(20)
	end
	app:type(field, "see ")
	pick("notes")
	pick("Look")
	app:key("Enter")
	app:frames(30)
	local sent = app:state("fixture_sent")
	local text = sent[#sent].text
	expect(text).to_contain(AM.link(1, "notes.md"))
	expect(text).to_contain("[Look into the flake](zeron-chat:s1)")
	-- What an agent reads: the file's name.
	expect(AM.prompt(text)).to_contain("see notes.md")
	-- The sent message: both chips, no link left as words.
	expect(app:error()).to_be_nil()
	-- The sent message, as the chat page draws it (the shell's page here
	-- draws no transcript): the same words, its file and chat with it.
	local page = mount("chat_fixture.luau")
	page:resize(1000, 800)
	page:command("go_s2")
	page:frames(30)
	page:set_state("chat.messages", {
		{ id = "dragged1", role = "user", text = text, attachments = { { kind = "file", name = "notes.md" }, { kind = "chat", name = "Look into the flake", chat = "s1" } } },
	})
	page:frames(30)
	local tree = page:tree("#dragged1")
	expect(tree).to_contain("see notes.md Look into the flake")
	expect(tree).never.to_contain("zeron-")
	expect(page:error()).to_be_nil()
end)
