-- When the composer's `/`, `@` and `$` list shows (ds/composer_mentions):
-- while the caret is in a token, the field has the keys, the window is at
-- the front and the composer is in view, unless that token's list was put
-- away (Escape, a click away that leaves the field the keys). Put away, it
-- stays away for that token alone: the caret leaving it, the token gone,
-- or a trigger typed starts a new one. Anything hiding it (Settings over
-- the page, a dialog over it, the field losing the keys) never puts it
-- away. On the chat page (ui/chat_fixture.luau) and the whole shell
-- (ui/shell_fixture.luau).
--
-- (The window going to the back isn't something a headless app can do: zgx
-- closes its popovers as it goes, as it does for a dialog opening over
-- them, which is the palette's case below.)

local TRIGGERS = {
	{ trigger = "/", word = "rev" },
	{ trigger = "@", word = "chat" },
	{ trigger = "$", word = "code" },
}

local function settle(app: any)
	app:frames(30)
end

local function chat(): any
	local app = mount("chat_fixture.luau")
	app:resize(900, 700)
	app:command("go_s2")
	settle(app)
	return app
end

local function shell(): any
	local app = mount("shell_fixture.luau")
	app:resize(1200, 800)
	settle(app)
	app:click('#shell-sidebar >> role:button "Fix the build"')
	settle(app)
	return app
end

-- (Shown: the popover in the tree and not hidden.)
local function shown(app: any): boolean
	for line in app:tree():gmatch("[^\n]+") do
		if line:find("mention-popover", 1, true) and not line:find(" hidden", 1, true) then
			return true
		end
	end
	return false
end

local function focused(app: any): string
	for line in app:tree():gmatch("[^\n]+") do
		if line:find(" focused", 1, true) then
			return line
		end
	end
	return ""
end

local BOX = 'role:textbox "Message"'

local function fresh(app: any, text: string)
	app:fill(BOX, text)
	settle(app)
end

test("1. put away with Escape, typed further at: it stays away", function()
	local app = chat()
	for _, t in TRIGGERS do
		fresh(app, "")
		app:type(BOX, t.trigger)
		settle(app)
		expect(shown(app)).to_be(true)
		app:key("Escape")
		settle(app)
		expect(shown(app)).to_be(false)
		app:type(BOX, string.sub(t.word, 1, 2))
		settle(app)
		expect(shown(app)).to_be(false)
	end
end)

test("2. put away, its trigger deleted and typed again: it opens", function()
	local app = chat()
	for _, t in TRIGGERS do
		fresh(app, "look at ")
		app:type(BOX, t.trigger)
		settle(app)
		app:key("Escape")
		settle(app)
		app:key("Backspace")
		settle(app)
		expect(shown(app)).to_be(false)
		app:type(BOX, t.trigger)
		settle(app)
		expect(shown(app)).to_be(true)
	end
end)

test("3. put away, a new trigger typed after it or elsewhere: it opens", function()
	local app = chat()
	for _, t in TRIGGERS do
		fresh(app, "")
		app:type(BOX, `look {t.trigger}{t.word}`)
		settle(app)
		app:key("Escape")
		settle(app)
		app:type(BOX, ` {t.trigger}`)
		settle(app)
		expect(shown(app)).to_be(true)
	end
end)

test("4. `/settings` taken with Enter, then `/`: the list opens", function()
	local app = chat()
	app:type(BOX, "/settings")
	settle(app)
	app:key("Enter")
	settle(app)
	expect(app:error()).to_be_nil()
	app:type(BOX, "/")
	settle(app)
	expect(shown(app)).to_be(true)
end)

test("6. a dialog over it (the palette) and closed: the list shows again, not put away", function()
	local app = shell()
	for _, t in { TRIGGERS[1], TRIGGERS[2] } do
		fresh(app, `testing {t.trigger}{t.word}`)
		expect(shown(app)).to_be(true)
		app:command("shell.palette")
		settle(app)
		expect(shown(app)).to_be(false)
		app:key("Escape")
		settle(app)
		expect(focused(app)).to_contain('textbox "Message"')
		expect(shown(app)).to_be(true)
	end
end)

test("7. the field losing the keys (a click away) and given them back in the token: the list shows", function()
	local app = chat()
	for _, t in TRIGGERS do
		fresh(app, `look {t.trigger}{t.word}`)
		expect(shown(app)).to_be(true)
		app:click('~"Hi! What shall we work on?"')
		settle(app)
		expect(shown(app)).to_be(false)
		app:type(BOX, "")
		settle(app)
		expect(shown(app)).to_be(true)
	end
end)

test("7. put away, the caret moved out of the token and back into it: the list shows", function()
	local app = chat()
	for _, t in TRIGGERS do
		fresh(app, `{t.trigger}{t.word} now`)
		-- (Into the token: back over " now".)
		for _ = 1, 4 do
			app:key("ArrowLeft")
		end
		settle(app)
		expect(shown(app)).to_be(true)
		app:key("Escape")
		settle(app)
		expect(shown(app)).to_be(false)
		app:key("ArrowRight")
		app:key("ArrowRight")
		settle(app)
		expect(shown(app)).to_be(false)
		app:key("ArrowLeft")
		app:key("ArrowLeft")
		settle(app)
		expect(shown(app)).to_be(true)
	end
end)

test("8. a row taken closes the list, and the next trigger opens it", function()
	local app = chat()
	for _, t in TRIGGERS do
		fresh(app, `{t.trigger}{t.word}`)
		expect(shown(app)).to_be(true)
		app:key("Enter")
		settle(app)
		expect(shown(app)).to_be(false)
		expect(app:text(BOX)).never.to_be(`{t.trigger}{t.word}`)
		app:type(BOX, t.trigger)
		settle(app)
		expect(shown(app)).to_be(true)
	end
end)

test("8. sent, the next trigger opens the list in the empty composer", function()
	local app = chat()
	fresh(app, "look in /tmp")
	app:key("Enter")
	settle(app)
	expect(app:text(BOX)).to_be("")
	app:type(BOX, "/")
	settle(app)
	expect(shown(app)).to_be(true)
end)

test("9. each chat's draft keeps its own: put away in one, another chat's opens, and back it's still away", function()
	local app = chat()
	fresh(app, "/rev")
	app:key("Escape")
	settle(app)
	expect(shown(app)).to_be(false)
	app:command("go_s1")
	settle(app)
	app:type(BOX, "/")
	settle(app)
	expect(shown(app)).to_be(true)
	app:key("Escape")
	app:fill(BOX, "")
	app:command("go_s2")
	settle(app)
	expect(app:text(BOX)).to_be("/rev")
	app:type(BOX, "")
	settle(app)
	expect(shown(app)).to_be(false)
	app:type(BOX, " /")
	settle(app)
	expect(shown(app)).to_be(true)
end)

-- (What row is lit, told by what Enter takes: the tree says nothing of a
-- row's `selected`.)
test("the lit row follows the list as it narrows, and the arrows move it", function()
	local app = chat()
	local function takes(): string
		app:key("Enter")
		settle(app)
		local got = app:text(BOX)
		fresh(app, "")
		return got
	end
	app:type(BOX, "/")
	settle(app)
	expect(takes()).to_be("/review ")
	app:type(BOX, "/comp")
	settle(app)
	expect(takes()).to_be("/compact ")
	app:type(BOX, "/")
	settle(app)
	app:key("ArrowDown")
	expect(takes()).to_be("/compact ")
	app:type(BOX, "/")
	settle(app)
	app:key("ArrowDown")
	app:type(BOX, "rev")
	settle(app)
	expect(takes()).to_be("/review ")
end)
