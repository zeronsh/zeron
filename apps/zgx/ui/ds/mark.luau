-- A harness's mark on its tile, as the intro and zeron.sh show them:
-- `Mark { name = "claude", size = 48 }`. Each is the vendor's own, bundled
-- (assets/icons): in its brand colour where the brand has one (Claude's
-- coral, Antigravity's gradient), one ink otherwise (OpenAI, Cursor: black
-- on the white tile, the text's colour bare). An agent from the ACP registry
-- is named by its harness (`acp.<id>`): its registry icon, found by that
-- alone (ds/registry_mark), so every place that names an agent's mark draws
-- a registry agent's too.

export type Name =
	"claude"
	| "openai"
	| "cursor"
	| "grok"
	| "devin"
	| "opencode"
	| "antigravity"
	| "pi"
	| "hermes"
	| "muse"

local FILES: { [Name]: string } = {
	claude = "claude-mark",
	openai = "openai-mark",
	cursor = "cursor-mark",
	grok = "grok-mark",
	devin = "devin-mark",
	opencode = "opencode-mark-dark",
	antigravity = "antigravity-mark.png",
	pi = "pi-mark",
	hermes = "hermes-mark",
	muse = "muse-mark",
}

-- Marks drawn in one ink (black, for the white tile): bare, they take the
-- colour of the text around them instead, or they vanish on dark.
local MONO: { [Name]: boolean } = { openai = true, grok = true, cursor = true, devin = true, hermes = true }

local RegistryMark = require("@self/registry_mark")

-- `bare`: the glyph alone, no tile (inline, beside text: a model's chip),
-- and then `name` may be a function the glyph follows. (`name` may be a
-- registry agent's harness, `acp.<id>`, too.)
export type Props = { name: Name | string | () -> (Name | string), size: number?, bare: boolean? }

local function registry(name: string): boolean
	return string.sub(name, 1, 4) == "acp."
end

-- The file of a mark (an SVG unless it names its own kind).
local function file(name: Name): string
	local f = FILES[name]
	return if string.find(f, "%.%a+$") then `assets/icons/{f}` else `assets/icons/{f}.svg`
end

-- A mark's picture at `side`: a raster drawn as it is, an SVG tinted when
-- it's one ink and `tint` is given.
local function picture(name: Name, side: number, tint: string?): Node
	local src = file(name)
	if string.sub(src, -4) == ".png" then
		return ui.image { src = src, w = side, h = side, shrink = 0, fit = "contain" }
	end
	return ui.svg { src = src, tint = if MONO[name] then tint else nil, w = side, h = side, shrink = 0 }
end

return function(props: Props): Node
	local size = props.size or 40
	local name = props.name
	if props.bare then
		if type(name) ~= "function" then
			if registry(name :: string) then
				return RegistryMark { harness = name :: string, size = size, bare = true }
			end
			return picture(name :: Name, size, "currentColor")
		end
		-- Following a name: a tinted glyph and a plain one, whichever the
		-- name's mark wants showing (a tint, once set, has no way back to
		-- the mark's own colours).
		return ui.presence(name :: any, function(now: string): Node
			if registry(now) then
				return RegistryMark { harness = now, size = size, bare = true }
			end
			return picture(now :: Name, size, "currentColor")
		end)
	end
	assert(type(name) == "string", "a tiled mark takes a name, not a function")
	if registry(name :: string) then
		return RegistryMark { harness = name :: string, size = size }
	end
	return ui.column {
		w = size,
		h = size,
		-- The tile's corner is a quarter of its size, whatever the size.
		rounded = ui.raw(size * 0.25, "a mark tile's radius scales with it"),
		items = "center",
		justify = "center",
		bg = "$zeron.mark.bg",
		border = 1,
		border_color = "$zeron.mark.border",
		shadow = "$shadow.sm",
		picture(name :: Name, size * 0.56, nil),
	}
end
