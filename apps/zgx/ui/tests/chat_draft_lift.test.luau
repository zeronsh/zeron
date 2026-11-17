-- A plugin allowed "draft" takes the composer's draft whole and puts it back
-- (`zeron.take_draft`, `put_draft`, `swap_draft`: screens/plugins keeps it
-- between, through screens/chat/shared's `lift` and `lay`). Taken, the
-- composer is clear; put back in another chat, its words and its chips are
-- as they were and go with the message; put back over words already there,
-- they follow those.

local function open(app: any, chat: string)
	app:command(`go_{chat}`)
	app:frames(30)
end

test("a draft taken in one chat is put back in another whole, chips and all", function()
	local app = mount("chat_fixture.luau")
	app:resize(900, 700)
	open(app, "s2")
	app:type('role:textbox "Message"', "Wrong chat prompt")
	app:command("mention_s1")
	app:frames(10)
	app:command("draft_lift")
	app:frames(10)
	expect(app:state("test_lifted")).to_be("Wrong chat prompt|1")
	-- Taken: nothing left on this chat's draft.
	expect(app:has('#chat-composer >> role:button "Long chat"')).to_be(false)
	expect(app:has('#chat-composer >> ~"Wrong chat prompt"')).to_be(false)
	-- Put back in another chat, and sent from there.
	open(app, "s3")
	app:command("draft_lay")
	app:frames(30)
	expect(app:has('#chat-composer >> ~"Wrong chat prompt"')).to_be(true)
	app:key("Enter")
	app:frames(30)
	expect(app:state("calls")).to_contain("send:s3:Wrong chat prompt\n\nChats attached:\n- [Long chat](zeron-chat:s1)")
	expect(app:error()).to_be(nil)
end)

test("an empty draft isn't taken", function()
	local app = mount("chat_fixture.luau")
	app:resize(900, 700)
	open(app, "s2")
	app:command("draft_lift")
	app:frames(10)
	expect(app:state("test_lifted")).to_be("nothing")
end)

test("a draft put back over words already there follows them", function()
	local app = mount("chat_fixture.luau")
	app:resize(900, 700)
	open(app, "s2")
	app:type('role:textbox "Message"', "Second")
	app:command("draft_lift")
	app:frames(10)
	app:type('role:textbox "Message"', "First")
	app:command("draft_lay")
	app:frames(30)
	app:key("Enter")
	app:frames(30)
	expect(app:state("calls")).to_contain("send:s2:First\n\nSecond")
	expect(app:error()).to_be(nil)
end)
