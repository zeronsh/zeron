-- Focus across a reload: a plain focusable button, and a ui.list's keyed rows.
local items = {}
for i = 1, 50 do
	items[i] = { id = `r{i}` }
end
local data = ui.state("probe_items", items)
return ui.column {
	w_full = true,
	h_full = true,
	ui.row { id = "plain", role = "button", label = "Plain", focusable = true, w = 100, h = 30, ui.text { "Plain" } },
	ui.list {
		id = "list",
		data = data,
		key = function(it: any): string
			return it.id
		end,
		height = 26,
		w = 300,
		h = 300,
		row = function(it: any): Node
			return ui.row { role = "treeitem", label = it.id, focusable = true, w_full = true, h = 26, ui.text { it.id } }
		end,
	},
}
