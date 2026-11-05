-- A measured list of 60 rows viewed at its top (only the first several
-- built), every other row a button, a field below it. ⇧Tab from the field
-- enters the list at its last BUILT row (r8), not its last row (r60), then
-- walks up to r2 and wraps round to the field: rows 10–60 are never
-- reached. Seen on zgx 2e22d99d3 and on 5b4893f9 (#1397). A browser's
-- ⇧Tab from below a long list goes to its last item. (Zeron meets it when
-- a chat reopens at its top: ⇧Tab from the composer then misses the end.)
local rows = {}
for i = 1, 60 do
	table.insert(rows, { id = `r{i}`, h = if i % 2 == 0 then 174 else 92 })
end
local data = ui.state("rows", rows)
local offset = ui.state("offset", 0)
return ui.column {
	w = 960,
	h = 700,
	ui.list {
		id = "list",
		w = 960,
		h = 614,
		data = data,
		key = function(r)
			return r.id
		end,
		measured = true,
		height = 96,
		overscan = 3,
		offset = offset,
		row = function(r)
			return ui.column { h = r.h, if r.h == 174 then ui.button { w = 300, h = 26, r.id } else ui.text { r.id } }
		end,
	},
	ui.input { id = "composer", w = 600, h = 22, value = "" },
}
