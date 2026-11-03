-- A file tree on files made up here, for tests/file_tree_in_place.test.luau:
-- two nested folders long enough to stay at the top as the list scrolls
-- past them. Its scroll is `test_tree_offset`. Here, beside app.luau, as a
-- mounted script loads modules from its own directory down.

ui.theme.strict("warn")
local ds = require("@ds")
ds.setup()
local FileTree = require("@ds/file_tree")

local lines: { string } = {}
for i = 1, 30 do
	table.insert(lines, `alpha/a{string.format("%02d", i)}.txt\t10\t`)
end
for i = 1, 30 do
	table.insert(lines, `alpha/beta/b{string.format("%02d", i)}.txt\t10\t`)
end
table.insert(lines, "zeta.txt\t10\t")
local model = FileTree.lines("fixture", table.concat(lines, "\n"))

return ui.column {
	w_full = true,
	h_full = true,
	FileTree {
		id = "tree",
		model = model,
		active = ui.state("test_tree_active", "alpha/beta/b30.txt" :: string?) :: any,
		offset = ui.state("test_tree_offset", 0) :: any,
		w_full = true,
		grow = 1,
	},
}
