-- The composer's `@` list as a launcher (ds/mention_launcher,
-- screens/chat/completions): the chat's files, its other chats and what the
-- draft carries, in sections under scopes (All, Files, Chats, Attachments)
-- that Tab and Shift+Tab go through; what's typed narrows every section;
-- each scope says when it has nothing. A pick inserts a file's mention, a
-- chat's, or a reference to what's attached, which goes out as
-- "(attachment: <its name>)" (its name alone once it's no longer attached).
-- On the chat page (ui/chat_fixture.luau).

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

-- The rows shown, in order, and the section headings among them.
local function rows(app: any): { string }
	local out = {}
	local inside = false
	for line in app:tree("#mention-launcher"):gmatch("[^\n]+") do
		if line:find('listbox "Results"', 1, true) then
			inside = true
		elseif inside and line:find('img "Up Arrow"', 1, true) then
			break
		elseif inside then
			local option = line:match('option "([^"]+)"')
			local heading = line:match('^%s*@%d+ text "(%u%a+)" %d+,%d+ %d+x16$')
			if option then
				table.insert(out, option)
			elseif heading then
				table.insert(out, `# {heading}`)
			end
		end
	end
	return out
end

local function count(app: any): string
	return app:tree("#mention-launcher"):match('text "(%d+ results?)"') or ""
end

test("`@` opens the launcher: what's typed and how many match, the scopes, the sections in order, the keys", function()
	local app = chat()
	app:command("drop_pictures")
	settle(app)
	app:type(BOX, "@")
	settle(app)
	local tree = app:tree("#mention-launcher")
	for _, tab in { "All", "Files", "Chats", "Attachments" } do
		expect(tree).to_contain(`tab "{tab}"`)
	end
	local shown = rows(app)
	local files, chats, attached = table.find(shown, "# Files"), table.find(shown, "# Chats"), table.find(shown, "# Attachments")
	expect(files ~= nil and chats ~= nil and attached ~= nil).to_be(true)
	expect((files :: number) < (chats :: number) and (chats :: number) < (attached :: number)).to_be(true)
	expect(shown).to_contain("zgx.png")
	expect(count(app)).never.to_be("")
	for _, key in { "Navigate", "Insert", "Scope" } do
		expect(tree).to_contain(`text "{key}"`)
	end
end)

test("Tab and Shift+Tab go through the scopes, each listing its own kind; the arrows move the light within it", function()
	local app = chat()
	app:command("drop_pictures")
	settle(app)
	app:type(BOX, "@")
	settle(app)
	app:key("Tab")
	settle(app)
	local files = rows(app)
	expect(files).to_contain("chat.rs")
	expect(files).never.to_contain("Long chat")
	expect(files).never.to_contain("# Files")
	app:key("Tab")
	settle(app)
	local chats = rows(app)
	expect(chats).to_contain("Long chat")
	expect(chats).never.to_contain("chat.rs")
	app:key("Tab")
	settle(app)
	local attached = rows(app)
	expect(attached).to_contain("zgx.png")
	expect(attached).never.to_contain("Long chat")
	-- Round to All, then back with Shift+Tab to Attachments.
	app:key("Tab")
	settle(app)
	expect(rows(app)).to_contain("# Chats")
	app:key("shift+Tab")
	settle(app)
	expect(rows(app)).never.to_contain("Long chat")
	expect(rows(app)).to_contain("zgx.png")
	-- The second row lit, then taken: the second attachment.
	app:key("ArrowDown")
	app:key("Enter")
	settle(app)
	expect(app:text(BOX)).to_contain("@photo.heic")
end)

test("what's typed narrows every section at once", function()
	local app = chat()
	app:command("drop_pictures")
	settle(app)
	app:type(BOX, "@long")
	settle(app)
	local shown = rows(app)
	expect(shown).to_contain("Long chat")
	expect(shown).to_contain("Long messages")
	expect(shown).never.to_contain("chat.rs")
	expect(shown).never.to_contain("zgx.png")
	expect(shown).never.to_contain("# Files")
	expect(count(app)).to_be(`{#shown - 1} results`)
end)

test("each scope says when it has nothing, and when nothing matches", function()
	local app = chat()
	app:type(BOX, "@")
	settle(app)
	for _ = 1, 3 do
		app:key("Tab")
	end
	settle(app)
	expect(app:tree("#mention-launcher")).to_contain("Nothing attached to this message")
	app:type(BOX, "zzzz")
	settle(app)
	expect(app:tree("#mention-launcher")).to_contain("No attachments match “zzzz”")
	app:key("shift+Tab")
	settle(app)
	expect(app:tree("#mention-launcher")).to_contain("No chats match “zzzz”")
end)

test("a file picked goes as its mention, a chat as Zeron's chat link", function()
	local app = chat()
	app:type(BOX, "@chat.r")
	settle(app)
	app:key("Enter")
	settle(app)
	app:type(BOX, "and @long-c")
	settle(app)
	app:key("Enter")
	settle(app)
	expect(app:text(BOX)).to_contain("@Long-chat")
	app:key("Enter")
	settle(app)
	expect(app:wait('~"and"', { timeout_ms = 2000 })).to_be(true)
	local calls = app:state("calls")
	expect(calls).to_contain("zeron-file:")
	expect(calls).to_contain("[Long chat](zeron-chat:s1)")
end)

test("what's attached, picked, goes as \"(attachment: <its name>)\"; once it's taken off, its name alone", function()
	local app = chat()
	app:command("drop_pictures")
	app:command("attach_passage")
	settle(app)
	app:type(BOX, "look at @zgx")
	settle(app)
	app:key("Enter")
	settle(app)
	expect(app:text(BOX)).to_contain("@zgx.png")
	app:key("Enter")
	settle(app)
	expect(app:wait('~"look at"', { timeout_ms = 2000 })).to_be(true)
	expect(app:state("calls")).to_contain("look at (attachment: zgx.png)")
	expect(app:state("calls")).never.to_contain("zeron-attachment")
	-- Taken off before it goes: the name alone.
	app:command("drop_pictures")
	settle(app)
	app:type(BOX, "then @photo")
	settle(app)
	app:key("Enter")
	settle(app)
	-- (Its × shows under the pointer.)
	app:hover('role:button "photo.heic"')
	app:frames(10)
	app:click('role:button "Remove photo.heic"')
	settle(app)
	-- (The keys back in the draft, at its end.)
	app:type(BOX, "")
	app:key("End")
	app:key("Enter")
	settle(app)
	expect(app:wait('~"then"', { timeout_ms = 2000 })).to_be(true)
	local calls = app:state("calls")
	expect(calls).to_contain("then photo.heic")
	expect(calls).never.to_contain("(attachment: photo.heic)")
end)

-- The bubble's words (its paragraph's text).
local function said(app: any, id: string): string
	return app:tree(`#{id}`):match('text "([^"]*Look at[^"]*)"') or app:tree(`#{id}`)
end

test("a prompt's \"(attachment: <name>)\" for what it carries is that one's chip; for what it doesn't, or other words, words", function()
	local app = chat()
	app:command("attachment_turn")
	settle(app)
	local words = said(app, "au1")
	-- (The chip shows its name alone.)
	expect(words).to_be("Look at zgx.png not (attachment: gone.png)")
	-- A prompt that carries nothing: its words as they are.
	expect(said(app, "au2")).to_be("Also Look at (attachment: zgx.png) here")
end)

test("round trip: an attachment picked in the draft goes as its words, and the sent bubble draws them as its chip", function()
	local app = chat()
	app:command("drop_pictures")
	settle(app)
	app:type(BOX, "Look at @zgx")
	settle(app)
	app:key("Enter")
	settle(app)
	app:key("Enter")
	settle(app)
	expect(app:wait('~"Look at"', { timeout_ms = 2000 })).to_be(true)
	expect(app:state("calls")).to_contain("Look at (attachment: zgx.png)")
	local tree = app:tree("#transcript")
	expect(tree).to_contain("Look at zgx.png")
	expect(tree).never.to_contain("(attachment: zgx.png)")
end)
