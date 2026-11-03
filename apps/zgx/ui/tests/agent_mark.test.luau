-- An agent's mark (ds/agent_mark, as ds/agent_look decides it): a built-in's
-- bundled brand mark; a registry agent's own icon from its entry, in the
-- theme's ink; the registry's glyph when the icon isn't there or isn't one;
-- and the board of them drawn in both themes without a fault.

local Look = require("@ds/agent_look")

local ICON = '<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 16 16"><path fill="currentColor" d="M0 0h16v16H0z"/></svg>'

test("Agent mark: a built-in agent draws its bundled brand mark", function()
	for _, name in { "claude", "openai", "cursor", "antigravity" } do
		local look = Look.look { mark = name }
		expect(look.kind).to_be("builtin")
		expect(look.mark).to_be(name)
	end
end)

test("Agent mark: a registry agent draws the icon from its entry, in the theme's ink", function()
	local tiled = Look.look { seed = "acp.gemini", icon = ICON }
	expect(tiled.kind).to_be("registry")
	expect(tiled.code).to_be(ICON)
	-- On its tile: theme tokens (they follow light and dark), never a colour
	-- of the agent's own.
	expect(tiled.tint).to_be(Look.INK)
	expect(string.sub(tiled.tint :: string, 1, 1)).to_be("$")
	expect(tiled.bg).to_be(Look.TILE)
	expect(string.sub(tiled.bg :: string, 1, 1)).to_be("$")
	-- Bare, beside text: the text's own colour, no tile.
	local bare = Look.look { seed = "acp.gemini", icon = ICON, bare = true }
	expect(bare.tint).to_be("currentColor")
	expect(bare.bg).to_be_nil()
	-- A bundled copy (the gallery's) the same way.
	local bundled = Look.look { seed = "gemini", src = "assets/icons/registry/gemini.svg" }
	expect(bundled.kind).to_be("registry")
	expect(bundled.src).to_be("assets/icons/registry/gemini.svg")
	expect(bundled.tint).to_be(Look.INK)
end)

test("Agent mark: no icon, or one that isn't an icon, falls back to the registry's glyph", function()
	for _, icon in { nil :: string?, "", "not an svg", "<svg" .. string.rep(" ", 300 * 1024) .. "/>" } do
		local look = Look.look { seed = "acp.factory-droid", icon = icon }
		expect(look.kind).to_be("fallback")
		expect(look.src).to_be(Look.FALLBACK)
		expect(look.tint).to_be(Look.INK)
	end
	-- One of your own: its initial.
	expect(Look.look({ seed = "acp.own.halcyon" }).kind).to_be("initial")
end)

test("Agent mark: the board draws in light and dark without a fault", function()
	local app = mount("agent_marks_fixture.luau")
	app:resize(1200, 900)
	app:frames(20)
	expect(app:error()).to_be_nil()
	expect(app:has(`#agent-marks >> "Factory Droid"`)).to_be(true)
end)
