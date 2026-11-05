-- Appshots (screens/chat/appshots), with the host's capture stubbed: a
-- capture the host publishes (`appshots.captured`) lands on the open
-- session's composer as a chip saying its app, opens in the lightbox, and
-- goes with the next send as a picture and its context; never sent on its
-- own; a capture that would pass 96 MB of staged Appshots says so; a
-- failure says why; and one taken in is never taken twice. On the chat page
-- of ui/chat_fixture.luau. Settings → Appshots reads the host's state.

local function settle(app: any)
	app:frames(30)
end

local function open(app: any, id: string)
	app:command(`go_{id}`)
	settle(app)
end

local SHOT = "assets/composer/shell.png"
local n = 0
local function capture(app: any, shot: any)
	n += 1
	local base = {
		id = `a{n}`,
		app = "Safari",
		bundle = "com.apple.Safari",
		title = "Release notes",
		-- (A real picture: headless, nothing asks whether a file is there.)
		path = SHOT,
		bytes = 1024,
	}
	for k, v in shot do
		base[k] = v
	end
	app:set_state("appshots.captured", { n = n, shot = base })
	settle(app)
end

test("a capture lands on the open session's composer as its app's chip, and isn't sent", function()
	local app = mount("chat_fixture.luau")
	app:resize(900, 700)
	open(app, "s2")
	capture(app, {})
	expect(app:error()).to_be_nil()
	expect(app:has('"Release notes"')).to_be(true)
	expect(app:state("calls") or "").never.to_contain("send:")
end)

test("sent, the capture goes as a picture and its context, the words first", function()
	local app = mount("chat_fixture.luau")
	app:resize(900, 700)
	open(app, "s2")
	capture(app, { text = "Version 2 ships Tuesday." })
	app:type("role:textbox", "What changed?")
	app:key("Enter")
	settle(app)
	expect(app:wait('~"What changed?"', { timeout_ms = 1000 })).to_be(true)
	settle(app)
	local calls = app:state("calls")
	expect(calls).to_contain("send:s2:What changed?")
	expect(calls).to_contain(`att:{SHOT}|upload`)
	expect(calls).to_contain(`appshot:Safari|Release notes|{SHOT}|text`)
end)

test("a capture past 96 MB of staged Appshots says so and stages nothing", function()
	local app = mount("chat_fixture.luau")
	app:resize(900, 700)
	open(app, "s2")
	capture(app, { title = "Big one", bytes = 90 * 1024 * 1024 })
	capture(app, { title = "One too many", bytes = 10 * 1024 * 1024 })
	expect(app:has('"Big one"')).to_be(true)
	expect(app:has('"One too many"')).to_be(false)
	expect(app:has('~"96 MB staged Appshot limit"')).to_be(true)
end)

test("a capture that failed says why", function()
	local app = mount("chat_fixture.luau")
	app:resize(900, 700)
	open(app, "s2")
	app:set_state("appshots.failed", { n = 1, message = "Allow window capture in System Settings." })
	settle(app)
	expect(app:has('"Couldn\'t capture an Appshot"')).to_be(true)
end)

test("a capture taken in is never taken twice (a reload keeps what was taken)", function()
	local app = mount("chat_fixture.luau")
	app:resize(900, 700)
	open(app, "s2")
	capture(app, { title = "Once only" })
	local once = app:count('#chat-composer >> ~"Once only"')
	expect(once > 0).to_be(true)
	-- The same capture published again (as a reload re-reads the host's).
	app:set_state("appshots.captured", { n = n, shot = { id = "again", app = "Safari", title = "Once only", path = SHOT } })
	settle(app)
	expect(app:count('#chat-composer >> ~"Once only"')).to_be(once)
end)

test("a capture's chip opens its picture in the lightbox", function()
	local app = mount("chat_fixture.luau")
	app:resize(900, 700)
	open(app, "s2")
	capture(app, { title = "Look here" })
	app:click('#chat-composer >> "Look here"')
	app:frames(60)
	expect(app:error()).to_be_nil()
	expect(app:state("ds/lightbox.luau::open")).never.to_be_nil()
	app:key("Escape")
	app:frames(60)
	expect(app:state("ds/lightbox.luau::open")).to_be_nil()
end)

-- Settings → Appshots, over the host's state as it publishes it.
local function settings(state: any): any
	local app = mount("shell_fixture.luau")
	app:resize(1200, 900)
	app:frames(30)
	app:command("settings_stub.sign_in")
	app:command("shell.settings")
	app:frames(20)
	app:set_state("appshots.state", state)
	app:click('#settings-nav >> role:tab "Appshots"')
	app:frames(30)
	expect(app:error()).to_be_nil()
	return app
end

test("Settings → Appshots says what each permission needs, and nothing once granted", function()
	local app = settings { platform = true, enabled = true, shortcut_ready = true, capture = "missing", text = "missing" }
	local page = app:tree("#settings-page")
	for _, words in { "Capture Appshots", "Capture sound", "Global shortcut", "Destination", "Window capture", "Application text", "Allow window capture", "Enable text capture", "Check again", "macOS asks for these for the app that opened Zeron" } do
		expect(if page:find(words, 1, true) then words else `missing: {words}`).to_be(words)
	end
	expect(page:find("⌃⌥Space", 1, true) ~= nil).to_be(true)
	app:set_state("appshots.state", { platform = true, enabled = true, shortcut_ready = true, capture = "ready", text = "ready" })
	app:frames(20)
	page = app:tree("#settings-page")
	expect(page:find("Allow window capture", 1, true)).to_be_nil()
	expect(page:find("Enable text capture", 1, true)).to_be_nil()
	expect(page:find("Ready", 1, true) ~= nil).to_be(true)
end)

test("Settings → Appshots records a new shortcut, and Escape keeps the old one", function()
	local app = settings { platform = true, enabled = true, shortcut_ready = true, capture = "ready", text = "ready" }
	local before = app:state("appshots_shortcut")
	app:click('#settings-page >> role:button ~"Change shortcut"')
	app:frames(10)
	expect(app:has('#settings-page >> "Press a shortcut…"')).to_be(true)
	app:key("Escape")
	app:frames(10)
	expect(app:state("appshots_shortcut")).to_be(before)
	app:click('#settings-page >> role:button ~"Change shortcut"')
	app:frames(10)
	app:key("cmd+shift+k")
	app:frames(10)
	expect(app:state("appshots_shortcut")).to_be("shift+cmd+k")
end)
