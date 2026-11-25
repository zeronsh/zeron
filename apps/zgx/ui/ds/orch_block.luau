-- A block of the Control room or an orchestrator's page: one widget as its
-- own unit, on the card settings' groups sit on (frosted, a quiet fill and
-- a hairline edge), so where one ends and the next begins is plain. Its
-- head: the title, its live figures after it, its menu at the end; its
-- body under it.
--
--   Block {
--     id = "control-widget-machines",
--     title = ui.t("control.widget.machines"),
--     figures = Node?,           -- after the title (counts, a live number)
--     menu = Node?,              -- at the head's end (a ⋯)
--     head = function(row) return ContextMenu { …, row } end,  -- wraps the head
--     flush = true,              -- the body runs to the card's edges (rows
--                                -- with their own inset, a list's hover)
--     body,
--   }
--
-- `Block.figure(text, tone?)`: a figure as the head shows one, small and
-- tabular, in the tertiary ink or a status's (`tone`: "failed", "input"…).
--
-- `Block.baseline(big, small)`: how far down to set a smaller text beside a
-- larger one in a row whose items are centred (zgx's rows centre boxes;
-- "baseline" sits them on their bottoms), so the two share a baseline:
-- `y = Block.baseline("$font.size.sm", "$font.size.xs")`, a move of its
-- paint alone. A centred line's baseline stands `K` of its size below its
-- middle, whatever its line height: K is Geist's (ascent − descent) / 2,
-- measured off a drawn row (0.38).

local material = require("@self/material")

local M = {}

-- The card's inset (one value: rule 6) and corner.
M.INSET = "$space.4"
local RADIUS = "$radius.xl"

export type Props = {
	id: string?,
	class: string?,
	title: any,
	figures: Node?,
	menu: Node?,
	head: ((row: Node) -> Node)?,
	flush: boolean?,
	gap: string?,
	[number]: Node?,
}

M.K = 0.38

local function size_of(token: string): number
	local v = ui.theme.get(string.sub(token, 2))
	return if type(v) == "number" then v else 13
end

-- The nudge down for a `small` text beside a `big` one (type size tokens).
function M.baseline(big: string, small: string): () -> number
	return function(): number
		ui.theme.current:get()
		return M.K * (size_of(big) - size_of(small))
	end
end

function M.figure(text: string | () -> string, tone: string?): Node
	return ui.text {
		text,
		tabular = true,
		shrink = 0,
		text_size = "$font.size.xs",
		color = if tone then `$zeron.status.{tone}` else "$text.tertiary",
	}
end

function M.Block(props: Props): Node
	local row = ui.row {
		class = "orch-block-head",
		w_full = true,
		h = 24,
		gap = "$space.2",
		items = "center",
		px = if props.flush then M.INSET else nil,
		ui.text {
			props.title,
			class = "orch-block-title",
			shrink = 0,
			text_size = "$font.size.sm",
			weight = "$font.weight.medium",
			color = "$text.primary",
		},
		-- (Its figures, smaller, on the title's baseline.)
		ui.row {
			class = "orch-block-figures",
			gap = "$space.2",
			items = "center",
			min_w = 0,
			y = M.baseline("$font.size.sm", "$font.size.xs"),
			props.figures,
		},
		ui.div { grow = 1 },
		props.menu,
	}
	local head = if props.head then props.head(row) else row
	local body: { any } = {}
	for _, child in ipairs(props :: any) do
		table.insert(body, child)
	end
	return ui.column {
		id = props.id,
		class = if props.class then `orch-block {props.class}` else "orch-block",
		w_full = true,
		rounded = RADIUS,
		bg = material.pick("$zeron.settings.tint", "$zeron.settings.card"),
		border = 1,
		border_color = "$zeron.settings.edge",
		p = if props.flush then nil else M.INSET,
		py = if props.flush then "$space.4" else nil,
		gap = props.gap or "$space.3",
		head,
		ui.column { class = "orch-block-body", w_full = true, min_w = 0, table.unpack(body) },
	}
end

return M
