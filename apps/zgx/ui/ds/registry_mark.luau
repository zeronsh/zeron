-- An agent from the ACP registry's mark, by its harness alone (`acp.<id>`):
-- its icon found where the app keeps every registry icon it has heard of
-- (`agent_icons`, src/agents.rs), so no caller has to carry it; until it's
-- there (or in a plugin, which can't read the app's values), the registry's
-- glyph. Drawn as ds/agent_look says: the theme's ink on a quiet tile, or
-- bare in the text's colour round it. ds.Mark and ds.AgentMark draw a
-- registry agent through this.
--
--   RegistryMark { harness = "acp.factory-droid", size = 14, bare = true }

local Look = require("@self/agent_look")

export type Props = {
	harness: string | () -> string,
	-- An icon the caller has already (drawn as given, not looked up).
	icon: string?,
	size: number?,
	bare: boolean?,
}

-- (The app's only: asking from a plugin raises, and a plugin draws the glyph.)
local icons_ok, icons = pcall(ui.shared, "agent_icons", {} :: { [string]: string })

local function icon_of(harness: string): string
	if not icons_ok then
		return ""
	end
	local id = if string.sub(harness, 1, 4) == "acp." then harness else `acp.{harness}`
	return ((icons :: any):get() :: { [string]: string })[id] or ""
end

-- One look, drawn.
local function draw(look: Look.Look, size: number, bare: boolean): Node
	if look.kind == "initial" then
		-- (One of your own, named by its harness alone: the glyph.)
		look = { kind = "fallback", src = Look.FALLBACK, tint = if bare then "currentColor" else Look.INK, bg = if bare then nil else Look.TILE }
	end
	local glyph = size * (if bare then 1 else 0.6)
	local svg = ui.svg {
		code = look.code,
		src = if look.code then nil else look.src,
		tint = look.tint,
		w = glyph,
		h = glyph,
		shrink = 0,
		aria_hidden = true,
	}
	-- (Its kind as a class: drawn from its icon, or the glyph in its place.)
	local class = if look.kind == "registry" then "registry-mark" else "registry-mark-glyph"
	if bare then
		return ui.div { class = class, shrink = 0, aria_hidden = true, svg }
	end
	-- The quiet tile, shaped as a built-in's (its corner a quarter of its
	-- size), so they sit in a row as one.
	return ui.column {
		w = size,
		h = size,
		shrink = 0,
		rounded = ui.raw(size * 0.25, "a mark tile's radius scales with it"),
		items = "center",
		justify = "center",
		bg = look.bg,
		class = class,
		aria_hidden = true,
		svg,
	}
end

return function(props: Props): Node
	local size = props.size or 32
	local bare = props.bare == true
	if props.icon then
		return draw(Look.look { icon = props.icon, bare = bare }, size, bare)
	end
	local harness = props.harness
	local function now(): string
		return if type(harness) == "function" then harness() else harness :: string
	end
	-- (Keyed on the icon itself: one heard of later takes the glyph's place.)
	return ui.presence(function(): string
		local h = now()
		return `{h}\0{icon_of(h)}`
	end, function(key: string): Node
		local h, svg = string.match(key, "^(.-)%z(.*)$")
		return draw(Look.look { seed = h, icon = if svg ~= "" then svg else nil, bare = bare }, size, bare)
	end)
end
