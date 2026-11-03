-- An agent from the ACP registry named outside Providers: every place draws
-- its registry icon, found by its harness (`acp.<id>`) alone, not the empty
-- slot or the glyph standing in for it. Factory Droid in ui/chat_fixture.luau
-- (s9: a chat moved from Devin to it), its icon among `agent_icons`.

local function settle(app: any)
	app:frames(30)
end

local function chat(): any
	local app = mount("chat_fixture.luau")
	app:resize(1100, 760)
	app:command("go_s9")
	settle(app)
	return app
end

test("Agent places: the switch line draws both sides' marks", function()
	local app = chat()
	local seam = '"Switched from Devin to Factory Droid"'
	expect(app:has(seam)).to_be(true)
	-- Factory Droid's from its registry icon, Devin's its own.
	expect(app:has(`{seam} >> .registry-mark`)).to_be(true)
	expect(app:has(`{seam} >> .registry-mark-glyph`)).to_be(false)
end)

local function shell(): any
	local app = mount("shell_fixture.luau")
	app:resize(1200, 800)
	settle(app)
	app:command("registry_session")
	settle(app)
	return app
end

test("Agent places: the sidebar's row and the palette's row carry it", function()
	local app = shell()
	expect(app:has('#sidebar >> role:button "Moved to Factory Droid" >> .registry-mark')).to_be(true)
	app:command("shell.palette")
	settle(app)
	app:type("#palette-search", "Moved")
	settle(app)
	expect(app:has('#palette-list >> role:option "Moved to Factory Droid" >> .registry-mark')).to_be(true)
end)

test("Agent places: the open chat's title carries its agent's mark", function()
	local app = shell()
	app:click('~"Moved to Factory Droid"')
	settle(app)
	expect(app:has("#page-title >> .registry-mark")).to_be(true)
end)

test("Agent places: a chat mentioned in the composer carries it", function()
	local app = chat()
	app:command("mention_s9")
	settle(app)
	expect(app:has("#chat-composer >> .registry-mark")).to_be(true)
end)

test("Agent places: a registry agent's mark waits for its icon, then draws it", function()
	local app = chat()
	-- (Its icon not heard of yet: the glyph holds the place, not an empty
	-- slot; then the icon, in place.)
	app:command("forget_icons")
	settle(app)
	local seam = '"Switched from Devin to Factory Droid"'
	expect(app:has(`{seam} >> .registry-mark-glyph`)).to_be(true)
	app:command("learn_icons")
	settle(app)
	expect(app:has(`{seam} >> .registry-mark`)).to_be(true)
end)

test("Agent places: a side chat's lane carries its agent's mark", function()
	local app = mount("lanes_fixture.luau")
	app:resize(1200, 900)
	settle(app)
	app:command("side_on_registry")
	settle(app)
	expect(app:has('#session-lanes >> role:button "Open side chat Looking at the tests" >> .registry-mark')).to_be(true)
end)

test("Agent places: the composer's model chip carries its agent's mark", function()
	local app = mount("chat_fixture.luau")
	app:resize(1100, 760)
	app:command("catalog_registry")
	app:command("go_s9")
	settle(app)
	expect(app:has('#chat-composer >> role:button "Model" >> .registry-mark')).to_be(true)
end)
