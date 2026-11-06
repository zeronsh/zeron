-- Keys: the one way the design system shows them, wherever it does (a
-- palette row's shortcut, a footer's legend, a menu item's keys). Two
-- looks: `cap`, each key in a cap of its own (⌘ then N), a quiet wash of the
-- text's ink; and `plain`, the keys as one run of text, as a menu writes
-- them (⇧⌘C). Two sizes, and three tones: `quiet` beside a label of its
-- own, `normal` in a legend, `disabled` on what can't be done now.
--
-- `Kbd { keys = "cmd+shift+n" }` (as `ui.command`'s keys: on macOS the
-- modifiers as symbols in its order, ⌃⌥⇧⌘, then the key; elsewhere
-- Ctrl+Shift+N) or `Kbd { text = "esc" }` (shown as written, one cap).
-- `tone` may be a function (a menu item's, following whether it is
-- enabled).

local locale = require("@self/locale")

-- The look of a cap (or of the plain run).
local recipe = ui.variants {
	base = {
		items = "center",
		justify = "center",
		shrink = 0,
		weight = "$font.weight.medium",
	},
	variants = {
		look = {
			cap = { px = "$space.1", rounded = "$radius.sm", bg = "$zeron.secondary.bg" },
			plain = {},
		},
		size = {
			sm = { h = 18, min_w = 18, text_size = "$font.size.xs" },
			md = { h = 20, min_w = 20, text_size = "$font.size.sm" },
		},
		tone = {
			quiet = { color = "$text.tertiary" },
			normal = { color = "$text.secondary" },
			disabled = { color = "$text.disabled" },
		},
	},
	-- Plain, the keys are text among text: its weight, no least width.
	compound = {
		{ look = "plain", min_w = 0, weight = "$font.weight.regular" },
	},
	defaults = { look = "cap", size = "sm", tone = "normal" },
}

export type Look = "cap" | "plain"
export type Size = "sm" | "md"
export type Tone = "quiet" | "normal" | "disabled"
export type Props = {
	keys: string?,
	text: string?,
	look: Look?,
	size: Size?,
	tone: (Tone | () -> Tone)?,
}

-- macOS's symbols: the modifiers in its order, then named keys ("Space"
-- the one word, shown in the locale's words: `shown`).
local MODS: { { string } } = { { "ctrl", "⌃" }, { "alt", "⌥" }, { "shift", "⇧" }, { "cmd", "⌘" } }
local NAMED: { [string]: string } = {
	enter = "↵",
	["return"] = "↵",
	backspace = "⌫",
	delete = "⌦",
	escape = "⎋",
	tab = "⇥",
	space = "Space",
	up = "↑",
	arrowup = "↑",
	down = "↓",
	arrowdown = "↓",
	left = "←",
	arrowleft = "←",
	right = "→",
	arrowright = "→",
}
-- Whether keys are written macOS's way here (the platform's own label for
-- ⌘A says).
local mac = string.find(ui.shortcut_text("cmd+a"), "⌘", 1, true) ~= nil

-- The keys, one string each: on macOS the held modifiers in its order,
-- then the key; elsewhere as the platform writes them, split at "+".
local function split(spec: string): { string }
	if not mac then
		local out: { string } = {}
		for part in string.gmatch(ui.shortcut_text(spec), "[^+]+") do
			table.insert(out, part)
		end
		return out
	end
	local held: { [string]: boolean } = {}
	local key: string? = nil
	for part in string.gmatch(string.lower(spec), "[^+]+") do
		if part == "option" then
			part = "alt"
		elseif part == "control" then
			part = "ctrl"
		end
		if part == "cmd" or part == "shift" or part == "alt" or part == "ctrl" then
			held[part] = true
		else
			key = NAMED[part] or string.upper(part)
		end
	end
	local out: { string } = {}
	for _, m in MODS do
		if held[m[1]] then
			table.insert(out, m[2])
		end
	end
	if key then
		table.insert(out, key)
	end
	return out
end

-- How each symbol is said, so a shortcut reads as one phrase ("Command
-- Shift C"), not a symbol at a time (each its message's key).
local SPOKEN: { [string]: string } = {
	["⌘"] = "ds.kbd.spoken.command",
	["⇧"] = "ds.kbd.spoken.shift",
	["⌥"] = "ds.kbd.spoken.option",
	["⌃"] = "ds.kbd.spoken.control",
	["↩"] = "ds.kbd.spoken.return",
	["⌫"] = "ds.kbd.spoken.delete",
	["⌦"] = "ds.kbd.spoken.forward_delete",
	["⎋"] = "ds.kbd.spoken.escape",
	["⇥"] = "ds.kbd.spoken.tab",
	["↑"] = "ds.kbd.spoken.up",
	["↓"] = "ds.kbd.spoken.down",
	["←"] = "ds.kbd.spoken.left",
	["→"] = "ds.kbd.spoken.right",
	Space = "ds.kbd.space",
}
local function spoken(parts: { string }): string
	local words: { string } = {}
	for _, p in parts do
		local key = SPOKEN[p]
		table.insert(words, if key then locale.s(key) else p)
	end
	return table.concat(words, " ")
end

-- A key as it's drawn: "Space" in the locale's words, the rest as written.
local function shown(part: string): string
	return if part == "Space" then locale.s("ds.kbd.space") else part
end

return function(props: Props): Node
	local look = props.look or "cap"
	local parts = if props.text then { props.text } else split(props.keys or "")
	local style = { look = look, size = props.size or "sm", tone = props.tone or "normal" } :: any
	-- (One label for assistive technology; the caps themselves hidden.)
	local function said(): string
		return spoken(parts)
	end
	if look == "plain" then
		return ui.row {
			variants = recipe(style),
			role = "img",
			label = said,
			ui.text {
				function(): string
					local out: { string } = {}
					for _, p in parts do
						table.insert(out, shown(p))
					end
					return table.concat(out, if mac then "" else "+")
				end,
				literal = true,
				aria_hidden = true,
			},
		}
	end
	local row: { [any]: any } = { gap = "$zeron.space.hair", items = "center", shrink = 0, role = "img", label = said }
	for _, key in parts do
		local text: any = if key == "Space" then locale.t("ds.kbd.space") else key
		table.insert(row, ui.row { variants = recipe(style), aria_hidden = true, ui.text { text, literal = true } })
	end
	return ui.row(row)
end
