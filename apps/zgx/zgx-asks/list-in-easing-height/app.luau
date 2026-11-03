-- A virtual list in a box whose height eases (a fold, as Zeron's sidebar
-- groups and panel lists fold). The box clips; a column in it holds the
-- list, which asks for its own height (12 rows of 200). Each row: a
-- fixed-height row holding a full-height row. `open` eases the box shut
-- (then unmounts it) or open again.
local rows = {}
for i = 1, 200 do
	table.insert(rows, { id = `r{i}`, title = `Row {i}` })
end
local data = ui.state("rows", rows)
local open = ui.state("open", true)
local amount = ui.state("amount", 1)
local mounted = ui.state("mounted", true)
local was = true
ui.effect(function()
	local on = open:get()
	if on == was then
		return
	end
	was = on
	if on then
		mounted:set(true)
	end
	anime.animate(amount, {
		value = if on then 1 else 0,
		duration = 240,
		ease = "out(3)",
		onComplete = function()
			if not open:peek() then
				mounted:set(false)
			end
		end,
	})
end)
local LIST_H = 12 * 32
return ui.column {
	w_full = true,
	ui.show(
		function(): boolean
			return mounted:get()
		end,
		ui.div {
			w_full = true,
			overflow = "hidden",
			h = function(): number
				return math.floor(LIST_H * amount:get() + 0.5)
			end,
			-- (Without this column, the list straight in the box, it lays out.)
			ui.column {
				w_full = true,
				ui.list {
					data = data,
					key = function(r: any): string
						return r.id
					end,
					row = function(_r: any, _i: State<number>, now: State<any>): Node
						-- (Without the inner `h_full` row, it lays out too.)
						return ui.row {
							w_full = true,
							h = 30,
							ui.row {
								w_full = true,
								h_full = true,
								ui.text { function(): string
									return now:get().title
								end },
							},
						}
					end,
					height = 32,
					w_full = true,
					h = LIST_H,
				},
			},
		}
	),
}
