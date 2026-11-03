-- A project's tile: its initial on its hue (the palette's 500, inked in its
-- 50). One tile wherever a project shows: the sidebar, a dropdown, the
-- composer's project.
--
-- `Tile { label = "Zeron", hue = "violet", size = 18 }`; `inside` puts a
-- glyph where the initial goes (pinned's pin), and `hue` may be nil for a
-- neutral tile (a quiet wash, the glyph in secondary ink). `src` is the
-- project's own artwork (an image its people chose, else its favicon, as
-- Zeron finds it in the checkout): shown whole in the tile's place, clipped
-- to its corner. `emoji` is a picture its people chose: the emoji in the
-- middle, an even inset all round, on a soft wash of its hue (none: the
-- neutral wash). Artwork wins over an emoji, an emoji over the initial.
-- `label`, `hue`, `src` and `emoji` may be functions (the tile of a choice
-- that changes): the tile follows them in place.

export type Props = {
	label: (string | Message | () -> string?)?,
	hue: (string | () -> string?)?,
	size: number?,
	inside: Node?,
	src: (string | () -> string?)?,
	emoji: (string | () -> string?)?,
}

-- Its corner grows a step with it: a sidebar's 18 takes the small radius,
-- a list row's 32 the next, a preview's 64 the large, so a big tile isn't a
-- square with chipped corners.
local control = require("@self/control")

local function corner(size: number): string
	return if size >= 56 then "$radius.xl" elseif size >= 32 then "$radius.md" else "$radius.sm"
end

-- An emoji's share of the tile: an even inset of about a fifth each side
-- on a big tile; a small one's emoji takes more of it (a glyph carries its
-- own side bearing, which at 7 points leaves a speck).
local EMOJI = 0.62
local EMOJI_SMALL = 0.8
-- How strongly an emoji tile's hue washes it: a tint under the emoji, not
-- the initial's solid fill (an emoji is in colour already).
local WASH = 0.22

local function read(v: (string | () -> string?)?): string?
	if type(v) == "function" then
		return v()
	end
	return v :: string?
end

return function(props: Props): Node
	local size = props.size or 18
	local radius = corner(size)
	local function artwork(): Node
		return ui.div {
			w = size,
			h = size,
			shrink = 0,
			rounded = radius,
			overflow = "hidden",
			ui.image {
				src = if type(props.src) == "function"
					then function(): string
						return (props.src :: () -> string?)() or ""
					end
					else props.src :: string,
				w_full = true,
				h_full = true,
				fit = "contain",
			},
		}
	end
	local function emoji(): Node
		local hue = props.hue
		return ui.div {
			w = size,
			h = size,
			shrink = 0,
			rounded = radius,
			overflow = "hidden",
			relative = true,
			items = "center",
			justify = "center",
			bg = "$zeron.secondary.bg",
			-- (The hue's own wash over the neutral one, so either theme's
			-- page shows through the same.)
			ui.div {
				absolute = true,
				left = 0,
				top = 0,
				w = size,
				h = size,
				bg = if type(hue) == "function"
					then function(): string
						local h = hue()
						return if h then `$palette.{h}.500` else "#00000000"
					end
					elseif hue then `$palette.{hue}.500`
					else "#00000000",
				opacity = WASH,
			},
			ui.text {
				if type(props.emoji) == "function"
					then function(): string
						return (props.emoji :: () -> string?)() or ""
					end
					else props.emoji :: string,
				text_size = ui.raw(math.floor(size * (if size < 32 then EMOJI_SMALL else EMOJI) + 0.5), "an emoji's glyph, in proportion to its tile"),
				line_height = size,
				h = size,
				align = "center",
				aria_hidden = true,
			},
		}
	end
	local function hued(): Node
		local hue = props.hue
		local function initial(): string
			-- (Its first character, whole: a name may start past ASCII.)
			local name = control.text(props.label) or ""
			local first = string.match(name, "^" .. utf8.charpattern) or ""
			return string.upper(first)
		end
		return ui.div {
			w = size,
			h = size,
			shrink = 0,
			rounded = radius,
			bg = if type(hue) == "function"
				then function(): string
					local h = hue()
					return if h then `$palette.{h}.500` else "$zeron.secondary.bg"
				end
				elseif hue then `$palette.{hue}.500`
				else "$zeron.secondary.bg",
			color = if type(hue) == "function"
				then function(): string
					local h = hue()
					return if h then `$palette.{h}.50` else "$text.secondary"
				end
				elseif hue then `$palette.{hue}.50`
				else "$text.secondary",
			items = "center",
			justify = "center",
			props.inside or ui.text {
				if type(props.label) == "function" or control.is_message(props.label) then initial else initial(),
				-- (A big tile's initial grows with it: half its side.)
				text_size = if size >= 32
					then ui.raw(math.floor(size * 0.46 + 0.5), "a big tile's initial, in proportion to it")
					else "$font.size.xs",
				weight = "$font.weight.semibold",
			},
		}
	end
	local src, glyph = props.src, props.emoji
	if type(src) == "function" or type(glyph) == "function" then
		-- (Which one shows is a real swap: each kind built once, kept while
		-- it shows, its values followed in place.)
		return ui.switch(function(): string
			local s, e = read(src), read(glyph)
			return if s and s ~= "" then "artwork" elseif e and e ~= "" then "emoji" else "hued"
		end, {
			artwork = if src then artwork() else ui.div {},
			emoji = if glyph then emoji() else ui.div {},
			hued = hued(),
		})
	end
	if src and src ~= "" then
		return artwork()
	end
	if glyph and glyph ~= "" then
		return emoji()
	end
	return hued()
end
