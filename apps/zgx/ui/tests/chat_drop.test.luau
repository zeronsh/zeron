-- Files carried over the chat from outside dim it under "Drop to attach"
-- until they drop or leave. A leave lost on the way (the platform's drag
-- ended elsewhere) must not leave it dimmed for good: Escape lets go of
-- them, as do the pointer back over the chat and a while with no word of
-- them. A sidebar session carried (the window's own drag) is kept.

local function open(app: any)
	app:resize(900, 700)
	app:command("go_s1")
	app:frames(30)
end

local function carried(app: any, what: any)
	app:set_state("chat.dropping", { session = what })
	app:frames(2)
end

test("Escape lets go of files carried over the chat", function()
	local app = mount("chat_fixture.luau")
	open(app)
	-- (The keys in the composer: the shell, not in this fixture, hears a
	-- bare Escape with nothing focused the same way.)
	app:click('role:textbox "Message"')
	app:frames(30)
	carried(app, true)
	expect(app:has("#chat-drop")).to_be(true)
	app:key("escape")
	app:frames(2)
	expect(app:has("#chat-drop")).to_be(false)
end)

test("the pointer back over the chat lets go of files carried over it", function()
	local app = mount("chat_fixture.luau")
	open(app)
	carried(app, true)
	expect(app:has("#chat-drop")).to_be(true)
	-- (A while after they came: a move from before can be heard late.)
	app:frames(30)
	app:hover("#chat-session")
	app:frames(2)
	expect(app:has("#chat-drop")).to_be(false)
end)

test("files carried with no word of them for a while are let go of", function()
	local app = mount("chat_fixture.luau")
	open(app)
	carried(app, true)
	app:frames(120)
	expect(app:has("#chat-drop")).to_be(true)
	app:frames(600)
	expect(app:has("#chat-drop")).to_be(false)
end)

test("a sidebar session carried over the chat is kept", function()
	local app = mount("chat_fixture.luau")
	open(app)
	carried(app, "session")
	expect(app:has("#chat-drop-mention")).to_be(true)
	app:frames(30)
	app:hover("#chat-session")
	app:frames(600)
	expect(app:has("#chat-drop-mention")).to_be(true)
end)
