-- An agent listing no models keeps its tab, its list saying why: still
-- looking, signed out ("Sign in"), or a look that failed ("Retry"). The
-- host once left such an agent out of `chat.catalog` and never looked
-- again, so Cursor, signed in after Zeron started or slow to answer, never
-- showed. Opening a picker asks the host to look again
-- (`chat.catalog_fresh`), an agent's "Retry" asks for its models alone
-- (`chat.catalog_retry`), and its models, come, list on its tab in place.
-- On the picker alone (ui/picker_cursor_fixture.luau), then in the chat's
-- composer (ui/chat_fixture.luau, its store's calls noted).

local function settle(app: any)
	app:frames(30)
end

-- The picker fixture, Cursor's models standing as `status` says.
local function picker(status: string): any
	local app = mount("picker_cursor_fixture.luau")
	app:resize(560, 640)
	app:set_state("test_cursor_status", status)
	settle(app)
	return app
end

test("Model catalog: an agent with no models keeps its tab, which says to sign in", function()
	local app = picker("empty")
	app:click('role:button "Model"')
	settle(app)
	expect(app:has('role:tab "Cursor"')).to_be(true)
	app:click('role:tab "Cursor"')
	settle(app)
	expect(app:has('~"NO MODELS — SIGN IN"')).to_be(true)
	expect(app:has('~"Sign in to Cursor to use its models"')).to_be(true)
	app:click('role:button "Sign in"')
	settle(app)
	expect(app:state("test_picker_calls")).to_contain("sign_in:cursor")
	expect(app:error()).to_be(nil)
end)

test("Model catalog: a look that failed says so, and Retry asks for that agent's models", function()
	local app = picker("error")
	app:click('role:button "Model"')
	settle(app)
	app:click('role:tab "Cursor"')
	settle(app)
	expect(app:has(`~"COULDN'T LOAD MODELS"`)).to_be(true)
	expect(app:has('~"not reachable"')).to_be(true)
	app:click('role:button "Retry"')
	settle(app)
	expect(app:state("test_picker_calls")).to_contain("retry:cursor")
	expect(app:error()).to_be(nil)
end)

test("Model catalog: an agent still looking holds its rows' shape, saying nothing's missing", function()
	local app = picker("loading")
	app:click('role:button "Model"')
	settle(app)
	app:click('role:tab "Cursor"')
	settle(app)
	expect(app:has('~"NO MODELS"')).to_be(false)
	expect(app:has('role:button "Retry"')).to_be(false)
	expect(app:error()).to_be(nil)
end)

test("Model catalog: opening the picker asks after the catalog", function()
	local app = picker("empty")
	expect(app:state("test_picker_calls")).to_be("")
	app:click('role:button "Model"')
	settle(app)
	expect(app:state("test_picker_calls")).to_be("open")
	expect(app:error()).to_be(nil)
end)

test("Model catalog: an agent's models, come, list on its tab in place", function()
	local app = picker("empty")
	app:click('role:button "Model"')
	settle(app)
	app:click('role:tab "Cursor"')
	settle(app)
	expect(app:has('role:option ~"Composer 2"')).to_be(false)
	app:set_state("test_cursor_status", "ready")
	settle(app)
	expect(app:has('role:tab "Cursor"')).to_be(true)
	expect(app:has('role:option ~"Composer 2"')).to_be(true)
	expect(app:has('role:option ~"Gemini 3.1 Pro"')).to_be(true)
	expect(app:has('~"NO MODELS — SIGN IN"')).to_be(false)
	expect(app:error()).to_be(nil)
end)

-- In the chat's composer: its store asked as the host would be.

local function composer(): any
	local app = mount("chat_fixture.luau")
	app:resize(960, 760)
	app:command("go_s2")
	settle(app)
	return app
end

-- Opened out on Cursor's tab (a press on the chosen model opens it out).
local function cursor_tab(app: any)
	app:click('#chat-composer >> role:button "Model"')
	settle(app)
	if not app:has('role:tab "Cursor"') or app:has(".folded") then
		app:click(".folded")
		settle(app)
	end
	app:click('role:tab "Cursor"')
	settle(app)
end

test("Model catalog: the composer's picker asks the host to look again as it opens", function()
	local app = composer()
	expect(app:state("calls")).never.to_contain("catalog_fresh")
	app:click('#chat-composer >> role:button "Model"')
	settle(app)
	expect(app:state("calls")).to_contain("catalog_fresh")
	expect(app:error()).to_be(nil)
end)

test("Model catalog: the composer's picker shows Cursor signed out, and its Retry after a failed look", function()
	local app = composer()
	app:command("catalog_cursor_empty")
	settle(app)
	cursor_tab(app)
	expect(app:has('~"NO MODELS — SIGN IN"')).to_be(true)
	app:command("catalog_cursor_error")
	settle(app)
	expect(app:has(`~"COULDN'T LOAD MODELS"`)).to_be(true)
	app:click('role:button "Retry"')
	settle(app)
	expect(app:state("calls")).to_contain("catalog_retry:cursor")
	expect(app:error()).to_be(nil)
end)

test("Model catalog: Cursor's models arriving in the catalog list on its tab", function()
	local app = composer()
	app:command("catalog_cursor_empty")
	settle(app)
	cursor_tab(app)
	expect(app:has('role:option ~"Composer 2"')).to_be(false)
	app:command("catalog_cursor_ready")
	settle(app)
	expect(app:has('role:tab "Cursor"')).to_be(true)
	expect(app:has('role:option ~"Composer 2"')).to_be(true)
	expect(app:has('~"NO MODELS — SIGN IN"')).to_be(false)
	expect(app:error()).to_be(nil)
end)
