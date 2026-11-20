-- What a plugin allowed "compose" puts in a chat's draft
-- (`zeron.compose(text, opts)`, handed on by screens/plugins as the host
-- asks it): after what's there, or in place of its words with its chips
-- kept (`replace`), in the chat shown or another (`session`), never sent.
-- On the shell in ui/shell_fixture.luau, the host's asks made here.

local seq = 0
local function ask(app: any, value: any)
	seq += 1
	app:set_state("plugins.request", { { seq = 1000 + seq, kind = "compose", title = "", value = value } })
	app:frames(10)
end

local function open(): any
	seq = 0
	local app = mount("shell_fixture.luau")
	app:resize(1400, 800)
	app:set_state("shell_fixture.plugins", true)
	app:frames(30)
	app:click('#shell-sidebar >> role:button "Fix the build"')
	app:frames(30)
	return app
end

local BOX = "#chat-composer >> role:textbox"

test("a plugin's words replace the draft's, or follow them on a line of their own", function()
	local app = open()
	app:type(BOX, "fix teh bulid pls")
	app:frames(5)
	ask(app, { text = "Please fix the build.", replace = true })
	expect(app:text(BOX)).to_be("Please fix the build.")
	ask(app, { text = "It fails on main." })
	expect(app:text(BOX)).to_be("Please fix the build.\nIt fails on main.")
	-- Words alone, as ever: after what's there.
	ask(app, "Thanks.")
	expect(app:text(BOX)).to_be("Please fix the build.\nIt fails on main.\nThanks.")
	-- Nothing sent.
	expect(app:state("calls")).never.to_contain("send:a1")
end)

test("a plugin's words go in another chat's draft, which shows them when it's opened", function()
	local app = open()
	app:type(BOX, "here, untouched")
	app:frames(5)
	ask(app, { text = "for the docs", replace = true, session = "a2" })
	expect(app:text(BOX)).to_be("here, untouched")
	app:click('#shell-sidebar >> role:button "Write the docs"')
	app:frames(30)
	expect(app:text(BOX)).to_be("for the docs")
	app:click('#shell-sidebar >> role:button "Fix the build"')
	app:frames(30)
	expect(app:text(BOX)).to_be("here, untouched")
end)
