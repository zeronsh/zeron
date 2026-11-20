-- A chat attached in the composer (a session dropped from the sidebar, its
-- card over the draft) is sent as an attachment: a card on the sent message
-- with the chat's title and its agent's mark, as the composer showed it, not
-- a mention in the words. Its link goes in a block after the words
-- (screens/chat/carried.luau `with_chats`), which the engine reads out to the
-- agent. A chat linked in the words themselves stays a chip in them.

local function open(app: any)
	app:resize(900, 700)
	app:command("go_s2")
	app:frames(30)
end

local function send(app: any, words: string)
	app:type('role:textbox "Message"', words)
	app:key("Enter")
	app:frames(30)
end

-- How many times `text` shows in the transcript's tree.
local function shown(app: any, text: string): number
	local _, n = string.gsub(app:tree("#transcript"), text, "")
	return n
end

test("a chat attached in the composer is a card on the sent message, not a mention in its words", function()
	local app = mount("chat_fixture.luau")
	open(app)
	app:command("mention_s1")
	app:frames(10)
	send(app, "Compare with")
	-- What's sent: the words, then the chat in its block, for the agent.
	local calls = app:state("calls")
	expect(calls).to_contain("send:s2:Compare with\n\nChats attached:\n- [Long chat](zeron-chat:s1)")
	-- The sent message: its words, and the chat as a card that opens it.
	expect(app:has('#transcript >> ~"Compare with"')).to_be(true)
	expect(app:has('#transcript >> role:button "Long chat"')).to_be(true)
	-- Its glyph on the card's tile (the agent's mark; the side chat's
	-- glyph for a chat whose agent this window doesn't know, as here).
	expect(app:tree('#transcript >> role:button "Long chat"')).to_contain("img")
	-- Once: the card, no chip in the words.
	expect(shown(app, "Long chat")).to_be(1)
	expect(app:error()).to_be(nil)
end)

test("a chat linked in the words stays a chip in them, with no card", function()
	local app = mount("chat_fixture.luau")
	open(app)
	send(app, "Compare with [Long chat](zeron-chat:s1) please")
	local calls = app:state("calls")
	expect(calls).to_contain("send:s2:Compare with [Long chat](zeron-chat:s1) please")
	expect(string.find(calls, "Chats attached:", 1, true)).to_be(nil)
	expect(app:has('#transcript >> role:button "Long chat"')).to_be(false)
	expect(app:error()).to_be(nil)
end)

