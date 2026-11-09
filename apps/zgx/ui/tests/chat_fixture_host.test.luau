-- The chat fixture run in the app itself (the real binary, its Rust host
-- installed, as `zgx inspect` drives it): the host also writes the chat's
-- shared states. It never opened the fixture's chat (the fixture's stubs
-- did), so an engine frame (WatchSessions, WatchChats, the engine going
-- unreachable) has it publish no session: src/chat.rs `publish_session`
-- writes nil. The fixture is the engine here, and keeps its chat open, as
-- an engine's chat stays open through its frames.

test("the fixture's chat stays open when the host publishes no session", function()
	local app = mount("chat_fixture.luau")
	app:resize(900, 820)
	app:command("go_s6")
	app:frames(20)
	app:command("many_long_messages")
	app:frames(10)
	app:scroll("#transcript", -3000)
	app:frames(5)
	expect(app:has("#transcript")).to_be(true)
	-- What the host does on an engine frame, with no chat of its own open.
	app:set_state("chat.session", nil)
	app:frames(10)
	expect(app:has("#transcript")).to_be(true)
	expect((app:state("chat.session") or {}).id).to_be("s6")
	expect(app:has('~"Message "')).to_be(true)
end)
