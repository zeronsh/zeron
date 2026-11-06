-- A side chat takes attachments as the chat page does (crates/ui's
-- `chat_dropzone`, around each side chat too): its + picks files, the whole
-- of its tab is a drop target for files and a sidebar session, what's carried
-- shows as chips on its own composer, and its message goes with them. None of
-- it touches the chat page's draft. On the page in ui/lanes_fixture.luau.

local function open(): any
	local app = mount("lanes_fixture.luau")
	app:resize(1200, 900)
	app:frames(30)
	return app
end

local function open_side(): any
	local app = open()
	app:click('#session-lanes >> role:button "Open side chat Looking at the tests"')
	app:frames(30)
	return app
end

local function calls(app: any): string
	return tostring(app:state("calls") or "")
end

local ATTACH = '.side-chat-tab >> role:button "Attach"'
local SEND = '.side-chat-tab >> role:button "Send message"'

test("a side chat's + attaches files as chips, and its message carries them", function()
	local app = open_side()
	app:write_file("notes.txt", "words")
	app:pick({ "notes.txt" })
	app:click(ATTACH)
	app:frames(10)
	expect(app:has('.side-chat-tab >> "notes.txt"')).to_be(true)
	app:type(".side-chat-tab >> role:textbox", "have a look")
	app:key("Enter")
	app:frames(10)
	expect(calls(app)).to_contain("send:side-a:have a look")
	expect(calls(app)).to_contain("attach:side-a:file:notes.txt")
	-- Sent: off the composer, on the prompt.
	expect(app:has('#lane-composer-tab1 >> "notes.txt"')).to_be(false)
	expect(app:has('.side-chat-tab >> "notes.txt"')).to_be(true)
end)

test("chips alone send", function()
	local app = open_side()
	app:write_file("notes.txt", "words")
	app:pick({ "notes.txt" })
	app:click(ATTACH)
	app:frames(10)
	app:click(SEND)
	app:frames(10)
	expect(calls(app)).to_contain("attach:side-a:file:notes.txt")
end)

test("a chip's × takes it off", function()
	local app = open_side()
	app:write_file("notes.txt", "words")
	app:pick({ "notes.txt" })
	app:click(ATTACH)
	app:frames(10)
	app:hover('.side-chat-tab >> role:button "notes.txt"')
	app:frames(10)
	app:click('.side-chat-tab >> role:button "Remove notes.txt"')
	app:frames(10)
	expect(app:has('.side-chat-tab >> "notes.txt"')).to_be(false)
end)

test("a new side chat's first message makes it and carries them", function()
	local app = open()
	app:click('#session-lanes >> role:button "New side chat"')
	app:frames(30)
	app:write_file("notes.txt", "words")
	app:pick({ "notes.txt" })
	app:click(ATTACH)
	app:frames(10)
	app:type(".side-chat-tab >> role:textbox", "first words")
	app:key("Enter")
	app:frames(30)
	local made = calls(app):match("create:chat%-1:([%w%-]+)")
	expect(made).never.to_be_nil()
	expect(calls(app)).to_contain(`send:{made}:first words:fresh`)
	expect(calls(app)).to_contain(`attach:{made}:file:notes.txt`)
end)

test("a message that doesn't go keeps its words and its chips", function()
	local app = open_side()
	app:write_file("notes.txt", "words")
	app:pick({ "notes.txt" })
	app:click(ATTACH)
	app:frames(10)
	app:command("send_fails")
	app:type(".side-chat-tab >> role:textbox", "have a look")
	app:key("Enter")
	app:frames(30)
	expect(app:has('#lane-composer-tab1 >> "notes.txt"')).to_be(true)
	expect(app:tree()).to_contain("have a look")
end)

test("a sidebar session dropped on a side chat is mentioned there, not on the chat page", function()
	local app = open_side()
	app:drag("#carry-session", ".side-chat-tab")
	app:frames(10)
	expect(app:has('#lane-composer-tab1 >> "Parent"')).to_be(true)
	expect(#(app:state("chat.composer.cargo") or {})).to_be(0)
	app:type(".side-chat-tab >> role:textbox", "see that one")
	app:key("Enter")
	app:frames(10)
	expect(calls(app)).to_contain("zeron-chat:chat-1")
end)

test("the files pane's files dropped on a side chat attach, by their path on its computer", function()
	local app = open_side()
	app:drag("#carry-paths", ".side-chat-tab")
	app:frames(10)
	expect(app:has('#lane-composer-tab1 >> "lib.rs"')).to_be(true)
	expect(#(app:state("chat.composer.cargo") or {})).to_be(0)
	app:click(SEND)
	app:frames(10)
	expect(calls(app)).to_contain("attach:side-a:file:lib.rs@")
end)

test("dropped on its composer, the same as on its transcript", function()
	local app = open_side()
	app:drag("#carry-paths", "#lane-composer-tab1")
	app:frames(10)
	expect(app:has('#lane-composer-tab1 >> "lib.rs"')).to_be(true)
end)

test("a side chat's chips are its own: another's composer shows none", function()
	local app = open_side()
	app:drag("#carry-paths", ".side-chat-tab")
	app:frames(10)
	app:click('#session-lanes >> role:button "Open side chat New side chat"')
	app:frames(30)
	expect(app:has('#lane-composer-tab2 >> "lib.rs"')).to_be(false)
	app:click('#session-lanes >> role:button "Open side chat Looking at the tests"')
	app:frames(30)
	expect(app:has('#lane-composer-tab1 >> "lib.rs"')).to_be(true)
end)

test("what's carried over a side chat dims that tab alone, and is let go of", function()
	local app = open_side()
	local function carried(what: any)
		app:set_state("chat.dropping_own", { ["lane.tab1"] = what })
		app:frames(2)
	end
	carried(true)
	expect(app:has("#lane-drop-tab1")).to_be(true)
	expect(app:has("#lane-drop-mention-tab1")).to_be(false)
	-- No word of them for a while: let go.
	app:frames(600)
	expect(app:has("#lane-drop-tab1")).to_be(false)
	carried(true)
	-- The pointer back over it, a moment after: the drag is over.
	app:frames(30)
	app:hover(".side-chat-tab")
	app:frames(2)
	expect(app:has("#lane-drop-tab1")).to_be(false)
	-- A sidebar session, the window's own drag: "Drop to mention", kept.
	carried("session")
	expect(app:has("#lane-drop-mention-tab1")).to_be(true)
	app:frames(30)
	app:hover(".side-chat-tab")
	app:frames(600)
	expect(app:has("#lane-drop-mention-tab1")).to_be(true)
end)
