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

-- A footer key's label as ds/kbd gives it: macOS's glyph (↑ said "Up
-- Arrow", ↵ as it is), elsewhere the platform's word ("Up", "Enter").
local function key_label(tree: string, mac: string, other: string): string
	return if tree:find(`img "{mac}"`, 1, true) then mac else other
end

-- The rows shown, in order, and the section headings among them.
local function rows(app: any): { string }
	local out = {}
	local inside = false
	for line in app:tree("#mention-launcher"):gmatch("[^\n]+") do
		if line:find('listbox "Results"', 1, true) then
			inside = true
		elseif inside and (line:find('img "Up Arrow"', 1, true) or line:find('img "Up"', 1, true)) then
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
	for _, key in { "Navigate", "Insert", "Switch tab" } do
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

test("what's attached, picked, goes as its mention (a picture by its number); once it's taken off, its name alone", function()
	local app = chat()
	app:command("drop_pictures")
	app:command("attach_passage")
	settle(app)
	app:type(BOX, "look at @zgx")
	settle(app)
	app:key("Enter")
	settle(app)
	-- (A picture: its token its number's, the third thing attached.)
	expect(app:text(BOX)).to_contain("@Image-3")
	app:key("Enter")
	settle(app)
	expect(app:wait('~"look at"', { timeout_ms = 2000 })).to_be(true)
	expect(app:state("calls")).to_contain("look at [Image 3](zeron-image:3)")
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
	-- (Taken off: its name alone, no link to nothing.)
	expect(calls).to_contain("then photo.heic")
	expect(calls).never.to_contain("](zeron-attachment:2)")
end)

-- The bubble's words (its paragraph's text).
local function said(app: any, id: string): string
	return app:tree(`#{id}`):match('text "([^"]*Look at[^"]*)"') or app:tree(`#{id}`)
end

-- (Messages sent before the mentions had links: their words as they were.)
test("an older prompt's \"(attachment: <name>)\" for what it carries is that one's chip; for what it doesn't, or other words, words", function()
	local app = chat()
	app:command("attachment_turn")
	settle(app)
	local words = said(app, "au1")
	-- (The chip shows its name alone.)
	expect(words).to_be("Look at zgx.png not (attachment: gone.png)")
	-- A prompt that carries nothing: its words as they are.
	expect(said(app, "au2")).to_be("Also Look at (attachment: zgx.png) here")
end)

test("round trip: an attachment picked in the draft goes as its mention, and the sent bubble draws it as its chip", function()
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
	expect(app:state("calls")).to_contain("Look at [Image 3](zeron-image:3)")
	local tree = app:tree("#transcript")
	expect(tree).to_contain("Look at Image 3")
	expect(tree).never.to_contain("zeron-image")
end)

-- Whether the launcher shows (in the tree and not hidden), and what has the
-- keys.
local function open_now(app: any): boolean
	for line in app:tree():gmatch("[^\n]+") do
		if line:find("#mention-popover", 1, true) then
			return not line:find(" hidden", 1, true)
		end
	end
	return false
end
local function field_has_keys(app: any): boolean
	for line in app:tree():gmatch("[^\n]+") do
		if line:find(" focused", 1, true) then
			return line:find('textbox "Message"', 1, true) ~= nil
		end
	end
	return false
end

test("a tab clicked switches to it and the launcher stays, the draft keeping the keys", function()
	local app = chat()
	app:command("drop_pictures")
	settle(app)
	app:type(BOX, "look @")
	settle(app)
	local wants = { Files = "chat.rs", Chats = "Long chat", Attachments = "zgx.png", All = "# Chats" }
	for _, tab in { "Files", "Chats", "Attachments", "All" } do
		app:click(`role:tab "{tab}"`)
		settle(app)
		expect(`{tab}: {open_now(app)}`).to_be(`{tab}: true`)
		expect(rows(app)).to_contain(wants[tab])
		expect(field_has_keys(app)).to_be(true)
		expect(app:text(BOX)).to_be("look @")
	end
	-- Still typed at: the query narrows the tab shown.
	app:key("Tab")
	app:frames(5)
	app:type(BOX, "")
	settle(app)
	expect(open_now(app)).to_be(true)
end)

test("a click on the launcher's blank room keeps it; a click on a row inserts it", function()
	local app = chat()
	app:type(BOX, "@")
	settle(app)
	for _, target in { '#mention-launcher >> "Navigate"', '#mention-launcher >> ~"results"', '#mention-launcher >> "Insert"' } do
		app:click(target)
		settle(app)
		expect(`{target}: {open_now(app)}`).to_be(`{target}: true`)
		expect(field_has_keys(app)).to_be(true)
	end
	app:click('role:option "Long chat"')
	settle(app)
	expect(app:text(BOX)).to_contain("@Long-chat")
	expect(field_has_keys(app)).to_be(true)
end)

-- Where what's at each row's end (its folder or kind, its project and
-- when it moved) ends, by row: its last text on the row's right half.
local function place_ends(app: any): { [string]: number }
	local out: { [string]: number } = {}
	local row: string? = nil
	for line in app:tree("#mention-launcher"):gmatch("[^\n]+") do
		local option = line:match('option "([^"]+)"')
		if option then
			row = option
		else
			local x, w = line:match('^%s*@%d+ text "[^"]*" (%-?%d+),%-?%d+ (%d+)x%d+$')
			-- (On the row's right half: not its name.)
			if row and x and tonumber(w) and (tonumber(w) :: number) > 12 and (tonumber(x) :: number) > 440 then
				out[row] = math.max(out[row] or 0, (tonumber(x) :: number) + (tonumber(w) :: number))
			end
		end
	end
	return out
end

-- (A row with no time has its place at the row's end, not in the middle;
-- one with a time, its time there: the same edge. No room is kept for ↵:
-- on the lit row it lies over that end, which fades out under it.)
test("every row is one line, what's at its end ending at the same place on each, and lighting a row moves nothing", function()
	local app = chat()
	app:type(BOX, "@")
	settle(app)
	local before = place_ends(app)
	local edge: number? = nil
	for name, ends in before do
		edge = edge or ends
		expect(`{name} ends at {ends}`).to_be(`{name} ends at {edge}`)
	end
	expect(edge ~= nil).to_be(true)
	app:key("ArrowDown")
	app:key("ArrowDown")
	settle(app)
	local after = place_ends(app)
	for name, ends in before do
		expect(`{name}: {after[name]}`).to_be(`{name}: {ends}`)
	end
	-- ↵ over the lit row's end: it ends where every row's text does.
	local tree = app:tree("#mention-launcher")
	local key = tree:match(`img "{key_label(tree, "↵", "Enter")}" (%-?%d+,%-?%d+ %d+x%d+)`)
	local x, w = (key or ""):match("^(%-?%d+),%-?%d+ (%d+)x")
	expect(`↵ ends at {(tonumber(x) or 0) + (tonumber(w) or 0)}`).to_be(`↵ ends at {edge}`)
end)
