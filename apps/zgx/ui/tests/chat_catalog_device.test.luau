-- The model picker lists the agents and models of the device the chat runs
-- on, as Zeron's does (crates/client `list_harnesses(device)`): the composer
-- tells the host which device that is as it changes (`store.catalog_on`).

test("a session on another device reads that device's catalog, and back", function()
	local app = mount("chat_fixture.luau")
	app:resize(900, 700)
	app:command("go_s2")
	app:frames(30)
	app:command("session_on_box")
	app:frames(30)
	expect(app:state("calls")).to_contain("catalog_on:box")
	-- Once: the same device again asks nothing more.
	local _, n = string.gsub(app:state("calls"), "catalog_on:box", "")
	app:command("session_on_box")
	app:frames(30)
	local _, again = string.gsub(app:state("calls"), "catalog_on:box", "")
	expect(again).to_be(n)
	expect(app:error()).to_be(nil)
end)

-- The connection, as Zeron's composer says it (composer.rs `link_notice`).
local LinkNotice = require("../screens/chat/link_notice")

test("the connection notice follows production's cases", function()
	local function text(link: any): string?
		local n = LinkNotice.of(link)
		return if n then n.text else nil
	end
	expect(text({})).to_be(nil)
	expect(text({ host_unreachable = true, host = "omarchy" })).to_be(nil)
	expect(text({ host_unreachable = true, host = "omarchy", intent = true })).to_be("Can't reach omarchy — messages will send when it's reachable.")
	expect(text({ connection = "reconnecting" })).to_be("Reconnecting…")
	expect(text({ connection = "reconnecting", host_unreachable = true, intent = true })).to_be("Reconnecting — messages will send once the connection recovers.")
	expect(text({ connection = "offline" })).to_be("Offline")
	expect(text({ connection = "offline", intent = true })).to_be("Offline — messages will send when you're back online.")
	expect(text({ connection = "unsaved" })).to_be(nil)
end)

test("the composer says when the connection or the chat's host is gone", function()
	local app = mount("chat_fixture.luau")
	app:resize(900, 700)
	app:command("go_s2")
	app:frames(30)
	app:command("connection_reconnecting")
	app:frames(10)
	expect(app:has('~"Reconnecting…"')).to_be(true)
	app:type('role:textbox "Message"', "Ship it")
	app:frames(10)
	expect(app:has('~"Reconnecting — messages will send once the connection recovers."')).to_be(true)
	app:command("connection_back")
	app:frames(10)
	expect(app:has('~"Reconnecting"')).to_be(false)
	-- On the old laptop, which has gone quiet: said only with a draft.
	app:command("session_on_old")
	app:frames(10)
	expect(app:has('~"Can\'t reach Old laptop — messages will send when it\'s reachable."')).to_be(true)
	expect(app:error()).to_be(nil)
end)
