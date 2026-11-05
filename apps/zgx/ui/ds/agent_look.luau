-- How an agent's mark is drawn (ds/agent_mark draws it; kept apart, with
-- no requires, for tests to read):
--
-- - an agent Zeron drives itself: its bundled brand mark (ds.Mark);
-- - an agent from the ACP registry: the registry's own icon for it (the
--   entry's `icon`, fetched by the engine from the registry's CDN and kept
--   on the device), in one ink: the theme's text colour, on a quiet tile, or
--   the text's colour round it, bare;
-- - one with no icon yet, or whose icon didn't load: the registry's glyph,
--   in the same ink (one of your own: its initial);
--
-- never a colour of the agent's own, a site's icon or an avatar.

export type Look = {
	-- "builtin" | "registry" | "fallback" | "initial"
	kind: string,
	-- The built-in's mark name ("builtin").
	mark: string?,
	-- The icon's SVG text, or a bundled file (`src`).
	code: string?,
	src: string?,
	-- Its ink: a theme token on the tile, `currentColor` bare.
	tint: string?,
	-- The tile under it (nil bare).
	bg: string?,
}

local M = {}

-- The registry's glyph, for an agent whose icon isn't there.
M.FALLBACK = "assets/icons/acp-registry.svg"
-- The ink on a tile (the theme's text colour) and the tile (its quiet
-- wash): both theme tokens, so a theme change redraws them.
M.INK = "$text.primary"
M.TILE = "$zeron.secondary.bg"

-- Whether an SVG is one an icon may be drawn from: an SVG, short of the
-- engine's cap. (The engine cleans what it fetches; this is the app's side.)
local MAX = 256 * 1024
local function drawable(svg: string?): boolean
	return type(svg) == "string" and #svg <= MAX and string.find(string.lower(svg), "<svg", 1, true) ~= nil
end

-- One of your own (`own.…`, `acp.own.…`): not from the registry.
local function own(seed: string?): boolean
	if not seed then
		return false
	end
	local id = string.gsub(seed, "^acp%.", "")
	return string.sub(id, 1, 4) == "own."
end

function M.look(props: { mark: string?, icon: string?, src: string?, seed: string?, bare: boolean? }): Look
	if props.mark then
		return { kind = "builtin", mark = props.mark }
	end
	local tint = if props.bare then "currentColor" else M.INK
	local bg = if props.bare then nil else M.TILE
	if drawable(props.icon) then
		return { kind = "registry", code = props.icon, tint = tint, bg = bg }
	end
	if props.src then
		return { kind = "registry", src = props.src, tint = tint, bg = bg }
	end
	if own(props.seed) then
		return { kind = "initial", bg = bg }
	end
	return { kind = "fallback", src = M.FALLBACK, tint = tint, bg = bg }
end

return M
