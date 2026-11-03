-- An agent's mark wherever agents are listed (Providers' cards, the
-- registry's rows, the model picker, the sidebar): Zeron's own mark for an
-- agent it drives itself (ds.Mark); else an agent from the ACP registry in
-- its real colours, or in plain ink when they aren't known. Never a colour
-- made up:
--
-- 1. its icon, when it carries colours of its own: drawn as authored;
-- 2. its real logo (its site's icon, or its organization's avatar), as the
--    app finds it (src/agent_logos.rs, `agent_logos`): one that is its own
--    tile fills the mark, clipped to its corner; a shape on transparency
--    sits inset on the white tile;
-- 3. else its one-ink icon in the ink round it (on the white tile, the
--    card's ink), as the registry draws it; with no icon, its initial on a
--    plain tile.
--
--   AgentMark { label = "Gemini CLI", seed = "gemini", icon = svg, size = 32 }
--   AgentMark { label = "Claude Code", mark = "claude", size = 32 }
--
-- `seed`: the agent's registry id (or its harness, `acp.<id>`): what its
-- logo is found by. `src`: a bundled file in place of `icon`. `bare`: the
-- glyph alone, beside text (a picker's row, a sidebar row). A logo found
-- later takes the icon's place where it is.

local Mark = require("@self/mark")
local Tile = require("@self/tile")

export type Props = {
	label: string,
	mark: string?,
	icon: string?,
	src: string?,
	seed: string?,
	size: number?,
	bare: boolean?,
}

local M = {}

-- The logos found, by registry id: { path, fill, source } (the host's).
-- (Only the app's: a plugin lent the design system can't read the app's
-- shared values, and asking raises. There, no logos: the plain mark.)
type Logos = { [string]: { path: string, fill: boolean, vector: boolean?, source: string } }
local logos_ok, logos_state = pcall(ui.shared, "agent_logos", {} :: Logos)
local logos = {
	get = function(_): Logos
		return if logos_ok then (logos_state :: any):get() else {}
	end,
}

-- A registry id from a seed (`acp.gemini` and `gemini` alike; none for one
-- of your own).
local function id_of(seed: string?): string?
	if not seed then
		return nil
	end
	local id = string.gsub(seed, "^acp%.", "")
	return if string.sub(id, 1, 4) == "own." then nil else id
end

-- Whether an SVG carries colours of its own (a fill, a stroke or a stop
-- that isn't `currentColor`, `none` or the like): drawn as authored.
local INKLESS = { currentcolor = true, none = true, transparent = true, inherit = true, [""] = true }
function M.authored(svg: string): boolean
	local text = string.lower(svg)
	for _, name in { "fill", "stroke", "stop%-color", "color" } do
		for value in string.gmatch(text, `[%s;"']{name}%s*[=:]%s*["']?([^"';%s>/]*)`) do
			if not INKLESS[value] and not string.match(value, "^url%(") then
				return true
			end
		end
	end
	return false
end

-- The white tile a mark sits on (ds.Mark's), round `inside`.
local function tile(size: number, inside: Node, color: string?): Node
	return ui.column {
		w = size,
		h = size,
		shrink = 0,
		-- (A mark tile's corner is a quarter of its size, as ds.Mark's.)
		rounded = ui.raw(size * 0.25, "a mark tile's radius scales with it"),
		items = "center",
		justify = "center",
		bg = "$zeron.mark.bg",
		border = 1,
		border_color = "$zeron.mark.border",
		shadow = "$shadow.sm",
		color = color,
		aria_hidden = true,
		inside,
	}
end

-- A logo that is its own tile: the whole mark, clipped to its corner, with
-- the tile's edge over it (a white or black logo keeps its edge on a page
-- of the same colour).
local function filled(path: string, size: number): Node
	local radius = ui.raw(size * 0.25, "a mark tile's radius scales with it")
	return ui.overlay {
		w = size,
		h = size,
		shrink = 0,
		rounded = radius,
		overflow = "hidden",
		-- (The white tile behind it: a logo with translucent parts reads the
		-- same on light and dark.)
		bg = "$zeron.mark.bg",
		shadow = "$shadow.sm",
		aria_hidden = true,
		ui.image { src = path, w = size, h = size, fit = "cover" },
		ui.div { w = size, h = size, rounded = radius, border = 1, border_color = "$zeron.mark.border" },
	}
end

function M.Mark(props: Props): Node
	local size = props.size or 32
	if props.mark then
		return Mark { name = props.mark :: any, size = size, bare = props.bare }
	end
	local bare = props.bare == true
	local glyph = size * (if bare then 1 else 0.56)
	local authored = props.icon ~= nil and M.authored(props.icon :: string)
	-- The registry's own icon (as authored, or in the ink round it).
	local function icon(): Node
		local svg: Node? = nil
		if props.icon then
			svg = ui.svg {
				code = props.icon,
				tint = if authored then nil else "currentColor",
				w = glyph,
				h = glyph,
				shrink = 0,
				aria_hidden = true,
			}
		elseif props.src then
			svg = ui.svg { src = props.src, tint = "currentColor", w = glyph, h = glyph, shrink = 0, aria_hidden = true }
		end
		if not svg then
			return Tile { label = props.label, size = size }
		end
		if bare then
			return svg :: Node
		end
		return tile(size, svg :: Node, if authored then nil else "$zeron.card.ink")
	end
	-- Its real logo, once found (none for one whose icon has colours of its
	-- own, or one of your own).
	local id = if authored then nil else id_of(props.seed)
	if not id then
		return icon()
	end
	local function found(): string
		local logo = logos:get()[id :: string]
		if not logo then
			return ""
		end
		local how = if logo.vector then "vector" elseif logo.fill then "fill" else "inset"
		return `{how}\0{logo.path}`
	end
	return ui.presence(found, function(key: string): Node
		if key == "" then
			return icon()
		end
		local how, path = string.match(key, "^(%a+)%z(.*)$")
		-- (A site's SVG: drawn as a vector, in its own colours.)
		local function picture(side: number): Node
			if how == "vector" then
				return ui.svg { src = path :: string, w = side, h = side, shrink = 0, aria_hidden = true }
			end
			return ui.image { src = path :: string, w = side, h = side, shrink = 0, fit = "contain", aria_hidden = true }
		end
		if how == "vector" then
			return if bare then picture(size) else tile(size, picture(size * 0.62))
		end
		if bare then
			-- (Beside text: a small white tile, the logo filling it or inset
			-- on it; a dark logo on transparency would vanish on a dark page.)
			local inner = if how == "fill" then size else size * 0.78
			return ui.column {
				w = size,
				h = size,
				shrink = 0,
				rounded = ui.raw(size * 0.25, "a mark's corner scales with it"),
				overflow = "hidden",
				items = "center",
				justify = "center",
				bg = "$zeron.mark.bg",
				aria_hidden = true,
				ui.image { src = path :: string, w = inner, h = inner, shrink = 0, fit = if how == "fill" then "cover" else "contain" },
			}
		end
		if how == "fill" then
			return filled(path :: string, size)
		end
		return tile(size, ui.image { src = path :: string, w = size * 0.62, h = size * 0.62, fit = "contain" })
	end)
end

-- Called as the component itself (`AgentMark { … }`).
return setmetatable(M, {
	__call = function(_self: any, props: Props): Node
		return M.Mark(props)
	end,
}) :: any
