-- Text kept private while the screen may be seen (shared, recorded): an
-- email address, drawn blurred while `PrivateText.hidden` is on.
--
--   PrivateText { "mara@halcyon.dev" }                    -- a string,
--   PrivateText { function() return me:get().email end } -- or a function
--
-- Hidden, the real text is never drawn, not even behind the blur: what's
-- drawn is a stand-in of the same length, the same for the same text every
-- time (its letters from a hash of it), with `@ . - _ +` kept where they
-- are so it still reads as an address. That stand-in is blurred, so even a
-- blur undone gives back only letters that were never the address, and it
-- can't be selected or copied. A click (or Enter) shows that one address
-- crisply; another hides it. The reveal is the
-- instance's own and isn't kept. Off, it's plain text.
--
-- Text props style it (`text_size`, `line_height`, `weight`, `color`), and
-- `truncate` fades its end where it doesn't fit (FadeText). Its height and
-- baseline are the text's in every state, so nothing moves as it switches.

local CardSeed = require("@self/card_seed")
local FadeText = require("@self/fade_text")
local locale = require("@self/locale")

local M = {}

-- Whether addresses are hidden: Settings → General's switch (kept across
-- launches), on until it's turned off. (Its own key: a switch saved off
-- before hiding was the default doesn't carry over.)
M.hidden = ui.persist("private_text_hidden", true)

-- The letters a stand-in is made of: no look-alikes (no i, l, o, 0, 1).
local ALPHABET = "abcdefghjkmnpqrstuvwxyz23456789"
-- What keeps its place: an address's shape.
local KEEP = "[@%.%-_+]"
-- The golden ratio's step (Weyl sequence), so each place draws its own.
local STEP = 0x9e3779b9
local TWO_32 = 0x100000000

-- The stand-in for `value`: as long as it, its separators where they are,
-- every other character one of ALPHABET picked by a hash of the value and
-- the place. Pure: the same value gives the same stand-in everywhere.
function M.stand_in(value: string): string
	local seed = CardSeed.hash(value)
	local out = table.create(#value)
	local n = #ALPHABET
	local place = 0
	for ch in string.gmatch(value, utf8.charpattern) do
		place += 1
		if string.find(ch, KEEP) then
			table.insert(out, ch)
		else
			local h = CardSeed.mix((seed + place * STEP) % TWO_32)
			local at = (h % n) + 1
			table.insert(out, string.sub(ALPHABET, at, at))
		end
	end
	return table.concat(out)
end

-- How far the stand-in is blurred: its letters gone, its shape kept.
local BLUR = 3

-- Props that style the text (the rest of a PrivateText's are its box's).
local TEXT = { text_size = true, line_height = true, weight = true, color = true, font = true, align = true }

export type Props = {
	[number]: any, -- the text: a string, or a function that returns one
	truncate: boolean?,
	-- False: a click doesn't show it (inside a row a press means something
	-- else to, a menu's). It stays hidden until the setting is off.
	reveal: boolean?,
	text_size: any?,
	line_height: any?,
	weight: any?,
	color: any?,
	-- What it's called to assistive technology while hidden ("Email"; a
	-- message too).
	name: (string | Message)?,
	-- Its box (the rest of a row's props: `grow`, `min_w`, margins…).
	grow: number?,
	shrink: number?,
	min_w: any?,
	max_w: any?,
}

-- The props that make the text, from a PrivateText's, with `content` for
-- its words.
local function text_of(props: Props, content: () -> string): Node
	local p: { [any]: any } = { content }
	for k, v in props :: any do
		if TEXT[k] then
			p[k] = v
		end
	end
	if props.truncate then
		p.grow = 1
		p.min_w = 0
		return FadeText(p)
	end
	return ui.text(p)
end

-- The props that place it (its box's): all but the text's own.
local function box_of(props: Props): { [any]: any }
	local p: { [any]: any } = {}
	for k, v in props :: any do
		if type(k) == "string" and not TEXT[k] and k ~= "truncate" and k ~= "name" and k ~= "reveal" then
			p[k] = v
		end
	end
	p.min_w = p.min_w or 0
	return p
end

-- One address, hidden: the blurred stand-in, or (shown) the address, a click
-- telling one from the other. Its own reveal, gone with it.
local Hidden = ui.component(function(p: { props: Props, value: () -> string }): Node
	local props, value = p.props, p.value
	local shown = ui.use_state(false)
	local function toggle()
		shown:set(not shown:peek())
	end
	local box = box_of(props)
	box.items = "center"
	-- (In a row that's pressed as a whole: no reveal, the press is the row's.)
	if props.reveal ~= false then
		-- A switch: on, the address shows (what it is said through `checked`).
		box.role = "switch"
		box.label = props.name or locale.t("ds.private_text.show_email")
		box.checked = function(): boolean
			return shown:get()
		end
		box.focusable = true
		box.cursor = "pointer"
		-- (No tooltip: the pointer cursor says it can be clicked.)
		box.on_click = toggle
		box.on_key = function(e: any): boolean
			if e.key == "Enter" or e.key == " " then
				toggle()
				return true
			end
			return false
		end
		box.focus = { outline = "$zeron.focus.width", outline_color = "$zeron.focus.ring", outline_offset = ui.raw(2, "the ring clear of the text's own ink") }
	end
	-- Which shows: the stand-in or the address (a real swap; the address
	-- arrives with a short fade, and goes at once).
	table.insert(
		box,
		ui.presence(function(): boolean
			return shown:get()
		end, function(is_shown: boolean): Node
			if is_shown then
				return text_of(props, value)
			end
			return ui.div {
				blur = BLUR,
				-- (Not to be selected, copied or read out: it isn't the
				-- address, and says nothing of it.)
				selectable = false,
				aria_hidden = true,
				min_w = 0,
				text_of(props, function(): string
					return M.stand_in(value())
				end),
			}
		end, { min_w = 0, enter = { opacity = { 0, 1 }, duration = 120 } })
	)
	return ui.row(box)
end)

-- A PrivateText: plain while addresses aren't hidden, blurred (Hidden)
-- while they are; switched as the setting changes, everywhere at once.
function M.Text(props: Props): Node
	local given: any = props[1]
	local function value(): string
		if type(given) == "function" then
			return (given :: () -> string?)() or ""
		end
		return (given :: string?) or ""
	end
	return ui.presence(function(): boolean
		return M.hidden:get()
	end, function(hidden: boolean): Node
		if hidden then
			return Hidden { props = props, value = value }
		end
		local box = box_of(props)
		table.insert(box, text_of(props, value))
		return ui.row(box)
	end, { min_w = 0 })
end

return setmetatable(M, {
	__call = function(_: any, props: Props): Node
		return M.Text(props)
	end,
})
