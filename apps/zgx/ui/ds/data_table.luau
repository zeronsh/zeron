-- A table of data laid straight on the page: no frame or fills, a quiet
-- header over a hairline, hairlines between rows, a row washed under the
-- pointer. Its first column's text lines up with the words around it; the
-- wash hangs past the text either side by a cell's padding. Columns share
-- the width evenly unless given one (`w`), each cell holding to one line.
-- For a reply's markdown table, see ds/markdown; this is for data a screen
-- or a visual lays out itself, whose cells may be any node.
--
--   DataTable {
--     columns = { { label = "Name" }, { label = "ms", align = "end" } },
--     rows = 12,
--     cell = function(r, c) return ui.text { … } end,
--   }

export type Column = { label: string?, align: ("start" | "center" | "end")?, w: number? }
export type Props = {
	columns: { Column },
	-- How many rows (and columns, if more than `columns` names).
	rows: number,
	count: number?,
	cell: (row: number, column: number) -> Node,
	hover: boolean?,
	label: string?,
}

local JUSTIFY = { start = "start", center = "center", ["end"] = "end" }

return function(props: Props): Node
	local columns = props.columns
	local count = math.max(props.count or 0, #columns)
	local hover = props.hover ~= false
	local space = ui.theme.get("space.3")
	local pad = if type(space) == "number" then space else 12
	local function cell(c: number, content: Node): Node
		local col = columns[c] or {}
		return ui.row {
			min_w = 0,
			grow = if col.w then 0 else 1,
			basis = if col.w then nil else 0,
			shrink = if col.w then 0 else 1,
			w = if col.w then ui.raw(col.w, "the column's width, as the data asks") else nil,
			px = "$space.3",
			py = "$space.2",
			items = "center",
			justify = JUSTIFY[col.align or "start"],
			overflow = "hidden",
			content,
		}
	end
	local out: { Node } = {}
	local labelled = false
	for _, col in columns do
		if col.label and col.label ~= "" then
			labelled = true
		end
	end
	if labelled then
		local head: { Node } = {}
		for c = 1, count do
			local col = columns[c] or {}
			table.insert(
				head,
				cell(
					c,
					ui.text {
						col.label or "",
						literal = true,
						truncate = true,
						text_size = "$zeron.visual.text.sm.size",
						line_height = "$zeron.visual.text.sm.line",
						weight = "$font.weight.medium",
						color = "$text.secondary",
					}
				)
			)
		end
		table.insert(out, ui.row { class = "data-table-head", w_full = true, head })
	end
	for r = 1, props.rows do
		if r > 1 or labelled then
			table.insert(out, ui.div {
				h = 1,
				shrink = 0,
				mx = "$space.3",
				bg = if r == 1 then "$border" else "$border.subtle",
			})
		end
		local cells: { Node } = {}
		for c = 1, count do
			table.insert(cells, cell(c, props.cell(r, c)))
		end
		table.insert(out, ui.row {
			class = "data-table-row",
			w_full = true,
			rounded = "$radius.md",
			hover = if hover then { bg = "$hover" } else nil,
			cells,
		})
	end
	return ui.column {
		class = "data-table",
		role = "group",
		label = props.label,
		-- (Out past the words' edge by a cell's padding, so the text lines up
		-- with them and the wash hangs past it.)
		mx = ui.raw(-pad, "a cell's padding: the first column's text on the words' edge"),
		self = "stretch",
		out,
	}
end
