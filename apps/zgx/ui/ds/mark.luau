-- A harness's mark on its tile, as the intro and zeron.sh show them:
-- `Mark { name = "claude", size = 48 }`.

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
	antigravity = "antigravity-mark",
	pi = "pi-mark",
	hermes = "hermes-mark",
	muse = "muse-mark",
}

-- Marks drawn in one ink (black, for the white tile): bare, they take the
-- colour of the text around them instead, or they vanish on dark.
local MONO: { [Name]: boolean } = { openai = true, grok = true, cursor = true, devin = true, hermes = true }

-- `bare`: the glyph alone, no tile (inline, beside text: a model's chip),
-- and then `name` may be a function the glyph follows.
export type Props = { name: Name | () -> Name, size: number?, bare: boolean? }

return function(props: Props): Node
	local size = props.size or 40
	local name = props.name
	if props.bare then
		if type(name) ~= "function" then
			return ui.svg {
				src = `assets/icons/{FILES[name]}.svg`,
				tint = if MONO[name] then "currentColor" else nil,
				w = size,
				h = size,
				shrink = 0,
			}
		end
		-- Following a name: a tinted glyph and a plain one, whichever the
		-- name's mark wants showing (a tint, once set, has no way back to
		-- the mark's own colours).
		local function src(): string
			return `assets/icons/{FILES[name()]}.svg`
		end
		local function mono(): boolean
			return MONO[name()] == true
		end
		return ui.div {
			shrink = 0,
			ui.show(mono, ui.svg { src = src, tint = "currentColor", w = size, h = size }),
			ui.show(function()
				return not mono()
			end, ui.svg { src = src, w = size, h = size }),
		}
	end
	assert(type(name) == "string", "a tiled mark takes a name, not a function")
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
		ui.svg { src = `assets/icons/{FILES[name :: Name]}.svg`, w = size * 0.56, h = size * 0.56 },
	}
end
