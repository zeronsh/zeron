-- A model's provider beside its name, where it tells namesakes apart: Pi
-- signed in to Codex and OpenCode lists GPT-5.5 from each, so those rows
-- say which (quiet, on the name's line, the name fading first when the row
-- runs short); a name only one provider has says nothing more, unless a
-- search found it by its provider ("codex"). On the picker in
-- ui/picker_sources_fixture.luau.

local function settle(app: any)
	app:frames(30)
end

-- Opened out on the chosen model's agent (Pi).
local function open(app: any)
	app:click('role:button "Model"')
	settle(app)
	app:click(".folded")
	settle(app)
end

-- A text's box (x, y, w, h) inside a row: the row's first line holding it.
local function box(app: any, row: string, text: string): { x: number, y: number, w: number, h: number }?
	local inside = false
	for line in app:tree():gmatch("[^\n]+") do
		if line:find(` option "{row}"`, 1, true) then
			inside = true
		elseif inside and line:find(" option ", 1, true) then
			return nil
		elseif inside and line:find(` text "{text}"`, 1, true) then
			local x, y, w, h = line:match("(%-?[%d%.]+),(%-?[%d%.]+) ([%d%.]+)x([%d%.]+)")
			return { x = tonumber(x) :: number, y = tonumber(y) :: number, w = tonumber(w) :: number, h = tonumber(h) :: number }
		end
	end
	return nil
end
local function row_box(app: any, row: string): { x: number, w: number }
	for line in app:tree():gmatch("[^\n]+") do
		if line:find(` option "{row}"`, 1, true) then
			local x, _, w = line:match("(%-?[%d%.]+),(%-?[%d%.]+) ([%d%.]+)x")
			return { x = tonumber(x) :: number, w = tonumber(w) :: number }
		end
	end
	return { x = -1, w = -1 }
end

local LONG = "GPT-5.5 Codex Max Preview Extended Context"

test("the provider shows only where two rows share a name", function()
	local app = mount("picker_sources_fixture.luau")
	app:resize(600, 700)
	settle(app)
	open(app)
	-- Pi's GPT-5.5 from each provider: each says which.
	expect(app:has('role:option "GPT-5.5 Codex" >> "Codex"')).to_be(true)
	expect(app:has('role:option "GPT-5.5 OpenCode" >> "OpenCode"')).to_be(true)
	-- A name one provider has: just its name.
	expect(app:has('role:option "GPT-5.5 mini"')).to_be(true)
	expect(app:has('role:option "Big Pickle"')).to_be(true)
	expect(app:has('role:option ~"GPT-5.5 mini" >> "Codex"')).to_be(false)
	expect(app:has('role:option ~"Big Pickle" >> "OpenCode"')).to_be(false)
	-- Quiet words of the name's size, on its line, the row's gap after it.
	local name = box(app, "GPT-5.5 Codex", "GPT-5.5") :: any
	local hint = box(app, "GPT-5.5 Codex", "Codex") :: any
	expect(name).never.to_be_nil()
	expect(hint).never.to_be_nil()
	expect(hint.y).to_be(name.y)
	expect(hint.h).to_be(name.h)
	expect(hint.x - (name.x + name.w)).to_be(8)

	-- An agent with no providers: none, though its GPT-5.5 shares Pi's name
	-- (its tab already says whose it is).
	app:click('role:tab "Claude"')
	settle(app)
	expect(app:has('role:option "GPT-5.5"')).to_be(true)
	expect(app:has('role:option "Opus"')).to_be(true)

	-- Starred, a row reads as it does on its agent's tab.
	app:set_state("test_picker_favorites", { ["pi/openai-codex/gpt-5.5"] = true, ["pi/opencode/big-pickle"] = true })
	app:click('role:tab "Starred"')
	settle(app)
	expect(app:has('role:option "GPT-5.5 Codex" >> "Codex"')).to_be(true)
	expect(app:has('role:option "Big Pickle"')).to_be(true)

	-- OpenCode signed out (its models gone from the catalog), in place: the
	-- Codex row has no namesake now, so it says just its name.
	local models = app:state("test_picker_models") :: { any }
	local kept = {}
	for _, m in models do
		if not (m.provider and m.provider.id == "opencode") then
			table.insert(kept, m)
		end
	end
	app:set_state("test_picker_models", kept)
	settle(app)
	expect(app:has('role:option "GPT-5.5"')).to_be(true)
	expect(app:has('role:option ~"GPT-5.5" >> "Codex"')).to_be(false)
	expect(app:error()).to_be(nil)
end)

test("a search finds models by their provider, and says it on each row", function()
	local app = mount("picker_sources_fixture.luau")
	app:resize(600, 700)
	settle(app)
	open(app)
	app:type('role:textbox "Search"', "opencode")
	settle(app)
	-- OpenCode's: each row says what it was found by.
	expect(app:has('role:option "GPT-5.5 OpenCode" >> "OpenCode"')).to_be(true)
	expect(app:has('role:option "Big Pickle OpenCode" >> "OpenCode"')).to_be(true)
	expect(app:has(`role:option "{LONG} OpenCode"`)).to_be(true)
	-- Codex's are gone.
	expect(app:has('role:option ~"GPT-5.5 mini"')).to_be(false)
	expect(app:has('role:option "GPT-5.5 Codex"')).to_be(false)
	expect(app:has(`role:option "{LONG} Codex"`)).to_be(false)

	-- A name and a provider read together.
	app:fill('role:textbox "Search"', "gpt-5.5 codex")
	settle(app)
	expect(app:has('role:option "GPT-5.5 Codex"')).to_be(true)
	expect(app:has('role:option "GPT-5.5 OpenCode"')).to_be(false)

	-- Found by its name, a row says no more than it did.
	app:fill('role:textbox "Search"', "pickle")
	settle(app)
	expect(app:has('role:option "Big Pickle"')).to_be(true)
	expect(app:has('role:option ~"Big Pickle" >> "OpenCode"')).to_be(false)

	-- A provider is found from a word's start, not inside one ("pen" is
	-- in OpenCode, but starts none of its words); "code" starts its second.
	app:fill('role:textbox "Search"', "pen")
	settle(app)
	expect(app:count("role:option")).to_be(0)
	app:fill('role:textbox "Search"', "code")
	settle(app)
	expect(app:has('role:option "Big Pickle OpenCode"')).to_be(true)

	-- An agent with no providers is searched by name alone.
	app:click('role:tab "Claude"')
	app:fill('role:textbox "Search"', "codex")
	settle(app)
	expect(app:count("role:option")).to_be(0)
	expect(app:error()).to_be(nil)
end)

test("a long name fades before its provider gives way", function()
	local app = mount("picker_sources_fixture.luau")
	app:resize(600, 700)
	settle(app)
	open(app)
	for _, p in { "Codex", "OpenCode" } do
		local row = `{LONG} {p}`
		local r = row_box(app, row)
		local name = box(app, row, LONG) :: any
		local hint = box(app, row, p) :: any
		local short = box(app, `GPT-5.5 {p}`, p) :: any
		expect(name).never.to_be_nil()
		expect(hint).never.to_be_nil()
		-- The hint whole (as wide as on a short row), inside the row's inset.
		expect(hint.w).to_be(short.w)
		expect(hint.x + hint.w <= r.x + r.w - 8).to_be(true)
		-- The name runs past where the hint starts: it is the one cut (its
		-- clip fades it there).
		expect(name.x + name.w > hint.x).to_be(true)
		expect(hint.x - name.x > 100).to_be(true)
	end
	expect(app:error()).to_be(nil)
end)

test("the keys pick a namesake by its provider", function()
	local app = mount("picker_sources_fixture.luau")
	app:resize(600, 700)
	settle(app)
	open(app)
	app:type('role:textbox "Search"', "opencode")
	settle(app)
	-- The first found, OpenCode's GPT-5.5, highlighted: Enter picks it.
	app:key("Enter")
	settle(app)
	expect(app:state("test_picker_model")).to_be("pi/opencode/gpt-5.5")
	-- Opened again, folded onto it: its row says which it is.
	if not app:has('role:option ~"GPT-5.5"') then
		app:click('role:button "Model"')
		settle(app)
	end
	expect(app:has('role:option "GPT-5.5 OpenCode" >> "OpenCode"')).to_be(true)
	-- Down from it, then Enter: the next row (Pi's list order).
	app:click(".folded")
	settle(app)
	app:key("ArrowDown")
	app:key("Enter")
	settle(app)
	expect(app:state("test_picker_model")).to_be("pi/openai-codex/gpt-5.5-mini")
	expect(app:error()).to_be(nil)
end)

test("a row gains its provider in place as the catalog names it", function()
	local app = mount("picker_sources_fixture.luau")
	app:resize(600, 700)
	-- An older engine's catalog first: no providers.
	local models = app:state("test_picker_models") :: { any }
	local bare = {}
	for _, m in models do
		local c = table.clone(m)
		c.provider = nil
		table.insert(bare, c)
	end
	app:set_state("test_picker_models", bare)
	settle(app)
	open(app)
	expect(app:has('role:option "GPT-5.5"')).to_be(true)
	expect(app:has('role:option ~"GPT-5.5" >> "Codex"')).to_be(false)
	-- Then one naming them, the panel open: the namesakes say which.
	app:set_state("test_picker_models", models)
	settle(app)
	expect(app:has('role:option "GPT-5.5 Codex" >> "Codex"')).to_be(true)
	expect(app:has('role:option "GPT-5.5 OpenCode" >> "OpenCode"')).to_be(true)
	expect(app:has('role:option "Big Pickle"')).to_be(true)
	expect(app:error()).to_be(nil)
end)
