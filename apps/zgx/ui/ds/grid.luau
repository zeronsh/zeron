-- A list laid as a grid: every row's name, cells and trailing slot on the
-- same lines, whatever their words. The name takes a fixed width, each cell a
-- column of its own whose figures end on its line (tabular), and the trailing
-- slot (an action, or why there's none) sits at the row's end. Short of room
-- a column goes from every row at once, below its `min`, so nothing's ever out
-- of line. The width goes by is the list's, measured once for all its rows.
--
--   local width = ui.use_state(0)
--   ui.column { on_resize = Grid.measure(width),
--     ui.row { Grid.name(grid, name), table.unpack(Grid.cells(grid, width, cells)), Grid.gap(), trail } }
--
-- The size lists use it (the deck's alert, ds/composer_dock; Settings → Cloud).

local FadeText = require("@self/fade_text")

local M = {}

export type Column = { w: number, min: number }
export type Grid = { name: number, columns: { Column } }

-- A list's width as it's laid out, into `width` (only as it changes).
function M.measure(width: State<number>): (size: { w: number, h: number }) -> ()
	return function(size: { w: number, h: number })
		if math.abs(width:peek() - size.w) >= 1 then
			width:set(size.w)
		end
	end
end

-- The name's column.
function M.name(grid: Grid, text: () -> string, props: { [string]: any }?): Node
	local node: { [any]: any } = {
		text,
		literal = true,
		w = ui.raw(grid.name, "the grid's name column"),
		shrink = 0,
		text_size = "$font.size.md",
		color = "$text.primary",
	}
	for k, v in props or {} do
		node[k] = v
	end
	return FadeText(node)
end

-- The cells' columns, each its words at its end, gone from every row while
-- the list is narrower than its `min`.
function M.cells(grid: Grid, width: State<number>, cells: () -> { string }?): { Node }
	local out: { Node } = {}
	for i, column in grid.columns do
		table.insert(out, ui.row {
			w = ui.raw(column.w, "the grid's column"),
			shrink = 0,
			justify = "end",
			hidden = function(): boolean
				local w = width:get()
				return w > 0 and w < column.min
			end,
			ui.text {
				function(): string
					local list = cells()
					return (list and list[i]) or ""
				end,
				tabular = true,
				text_size = "$font.size.sm",
				color = "$text.tertiary",
			},
		})
	end
	return out
end

-- What pushes the trailing slot to the row's end.
function M.gap(): Node
	return ui.div { grow = 1, min_w = 0 }
end

-- The trailing slot's words (why there's no action, or "Current"), at the
-- row's end where an action would be. `strong`: in the primary ink.
function M.trail(text: () -> string?, strong: (() -> boolean)?): Node
	return ui.show(function(): boolean
		return text() ~= nil
	end, ui.text {
		function(): string
			return text() or ""
		end,
		shrink = 0,
		pr = "$space.2",
		text_size = "$font.size.xs",
		color = function(): string
			return if strong and strong() then "$text.primary" else "$text.secondary"
		end,
	})
end

return M
