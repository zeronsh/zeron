-- A message another chat sent: `Message from [mark] Chat` over its bubble, the
-- chat's title as it's listed now, a click opening it in the panel; one no longer listed
-- reads as it did when sent, and opens nothing. On ui/chat_fixture.luau.

local CHATS = "account/sessions.luau::sessions"

local function open(): any
	local app = mount("chat_fixture.luau")
	app:resize(1000, 800)
	app:frames(30)
	app:command("messages_from_chats")
	app:frames(30)
	return app
end

test("a message from a chat that isn't listed names it as it was sent, and opens nothing", function()
	local app = open()
	local tree = app:tree("#transcript")
	expect(tree).to_contain("Message from")
	expect(tree).to_contain("Gone chat")
	expect(tree).to_contain("Waiting on you.")
	expect(app:has('role:button ~"Gone chat"')).to_be(false)
end)

test("no routing header in what shows", function()
	local app = open()
	local tree = app:tree("#transcript")
	expect(string.find(tree, "Reply to it with", 1, true)).to_be(nil)
end)

test("a message from a listed chat reads its title now and opens it", function()
	local app = open()
	local tree = app:tree("#transcript")
	expect(tree).to_contain("Short chat")
	expect(string.find(tree, "Old title", 1, true)).to_be(nil)
	app:click('role:button "Message from Short chat"')
	app:frames(40)
	-- A tab of the panel, beside this chat, which stays.
	local tabs = 0
	for _, s in (app:state("panel_sessions") :: any) or {} do
		for _, tab in s.tabs do
			if tab.kind == "side_chat" and tab.title == "Short chat" then
				tabs += 1
			end
		end
	end
	expect(tabs).to_be(1)
	expect(app:has('role:button "Message from Short chat"')).to_be(true)
end)
