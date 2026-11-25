--!strict
-- The Control room's card look, shared: the board's cards (ds/orch_board)
-- and whatever else stands in its columns. A card is calm: a title of two
-- lines at most, one quiet line under it whose parts each show only where
-- they say something (dot-separated), and its "…" menu where the pointer
-- rests.
--
--   local Card = require("@ds/orch_card")
--   Card.RADIUS, Card.pad(compact), Card.bg(hovered)   -- the box
--   Card.Title { text = fn, quiet = fn -> bool }       -- two lines at most
--   Card.Line { class?, parts... }                     -- the quiet line
--   Card.Word { text = fn, ink = fn?, strong = bool? } -- a part's words
--   Card.Sep(shown)                                    -- room between parts
--   Card.Menu { items = fn, open = state, shown = fn, label = string }

local ContextMenu = require("@self/context_menu")
local IconButton = require("@self/icon_button").IconButton
local material = require("@self/material")

local M = {}

M.RADIUS = "$radius.lg"

-- Its even inset: denser in a page's side pane.
function M.pad(compact: boolean?): any
	return if compact then "$space.2" else ui.raw(10, "a card's even inset")
end

-- Its fill: the hover's while the pointer rests on it.
function M.bg(hovered: () -> boolean): () -> string
	local rest = material.pick("$zeron.settings.note", "$zeron.settings.card")
	return function(): string
		return if hovered() then "$hover" else rest()
	end
end

-- Its title: two lines at most (a long one clamped), quieter when `quiet`.
function M.Title(props: { text: () -> string, quiet: (() -> boolean)? }): Node
	return ui.text {
		props.text,
		class = "orch-card-title",
		wrap = true,
		lines = 2,
		truncate = true,
		w_full = true,
		min_w = 0,
		text_size = "$font.size.sm",
		line_height = "$font.line.sm",
		weight = "$font.weight.medium",
		color = function(): string
			return if props.quiet and props.quiet() then "$text.secondary" else "$text.primary"
		end,
	}
end

-- The quiet line: its parts in a row, one line high.
function M.Line(props: { [any]: any }): Node
	local line = (ui.theme.get("font.line.sm") :: number?) or 16
	local p: { [any]: any } = {
		class = props.class or "orch-card-status",
		w_full = true,
		min_w = 0,
		h = line,
		gap = "$space.1",
		items = "center",
	}
	for i, child in ipairs(props) do
		p[i] = child
	end
	return ui.row(p)
end

-- A part's words, small; `strong` for a priority's.
function M.Word(props: { text: () -> string, ink: (() -> string)?, strong: boolean?, class: string?, shrink: number? }): Node
	return ui.text {
		props.text,
		class = props.class,
		min_w = 0,
		shrink = props.shrink,
		truncate = true,
		tabular = true,
		text_size = "$font.size.xs",
		weight = if props.strong then "$font.weight.medium" else nil,
		color = props.ink or "$text.tertiary",
	}
end

-- Room between two parts, while both show: spacing, no typed dot.
function M.Sep(shown: () -> boolean): Node
	return ui.show(shown, ui.div { class = "orch-card-sep", w = "$space.1", shrink = 0 })
end

-- Its "…": shown where the pointer rests (`shown`) or while open, the
-- same entries as its right-click.
-- (Built the first time it shows: a board of cards would otherwise build
-- one for every card, most never pointed at.)
function M.Menu(props: { items: () -> { any }, open: any, shown: () -> boolean, label: any }): Node
	local function on(): boolean
		return props.shown() or props.open:get()
	end
	return ui.show(on, ui.presence(function(): string?
		return if on() then "on" else nil
	end, function(): Node
		return M.MenuButton(props)
	end))
end

function M.MenuButton(props: { items: () -> { any }, open: any, shown: () -> boolean, label: any }): Node
	return (ContextMenu {
		items = props.items,
		open = props.open,
		label = props.label,
		shrink = 0,
		IconButton {
			class = "orch-card-menu",
			icon = "more-horizontal",
			label = props.label,
			size = "sm",
			tone = "quiet",
			lit = props.open,
			on_click = function()
				props.open:set(true)
			end,
		},
	}) :: Node
end

return M
