-- The composer's draft as plugins read it (`zeron.draft`, "draft"): written
-- only while a plugin that's on may read it, so with none no key typed
-- copies the draft for no one; with one, it follows what's typed, its chat
-- and its chips by kind and name. On the shell in ui/shell_fixture.luau.

local function open(): any
	local app = mount("shell_fixture.luau")
	app:resize(1400, 800)
	app:set_state("shell_fixture.plugins", true)
	app:frames(30)
	app:click('#shell-sidebar >> role:button "Fix the build"')
	app:frames(30)
	return app
end

local function with_plugin(app: any, status: string)
	app:set_state("plugins", {
		state = "ready",
		list = { { id = "stash", name = "Stash", status = status, permissions = { "ui.add", "draft" }, allowed = { "ui.add", "draft" } } },
	})
	app:frames(5)
end

test("with no plugin that reads the draft, typing publishes nothing", function()
	local app = open()
	app:type("#chat-composer >> role:textbox", "a private thought")
	app:frames(5)
	expect(app:state("zeron.draft")).to_be_nil()
	-- One that's off reads nothing either.
	with_plugin(app, "off")
	app:type("#chat-composer >> role:textbox", "still private")
	app:frames(5)
	expect(app:state("zeron.draft")).to_be_nil()
end)

test("with a plugin on that reads the draft, it follows what's typed, and goes with the plugin", function()
	local app = open()
	with_plugin(app, "on")
	app:type("#chat-composer >> role:textbox", "first words")
	app:frames(5)
	local d = app:state("zeron.draft")
	expect(d).never.to_be_nil()
	expect(d.text).to_be("first words")
	expect(type(d.session)).to_be("string")
	expect(#d.attachments).to_be(0)
	-- Turned off: nothing kept out for it.
	with_plugin(app, "off")
	expect(app:state("zeron.draft")).to_be_nil()
end)
