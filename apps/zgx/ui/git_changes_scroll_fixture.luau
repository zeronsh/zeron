-- Long lines outside the mounted slice must still set the horizontal range.
local ds = require("@ds")
ds.setup()
local Changes = require("@ds/git_changes")
local git = require("@ds/git")
local wrap = ui.state("test_diff_wrap", false)
local split = ui.state("test_diff_split", false)
local opened = ui.state("test_diff_open", { ["src/scroll.rs"] = true })
local bare = ui.state("test_diff_bare", false)
local revision = ui.state("test_diff_short", false)
local full = { "@@ -1,200 +1,200 @@ fn scroll() {", " first visible line", "-old line", "+new line" }
for i = 1, 195 do table.insert(full, ` line {i}`) end
table.insert(full, "+\t" .. string.rep("long 界 ", 90) .. "END")
local long = git.parse(full)
local short = git.parse({ "@@ -1 +1 @@", " short" })
local file = Changes.file("src/scroll.rs", "modified", 2, 1)
return ui.column {
	w_full = true, h_full = true,
	-- A second instance covers the bare (wide viewer) path with the same data.
	ui.presence(function(): string return if bare:get() then "bare" else "inline" end, function(mode: string): Node
		return Changes.list {
			id = "scroll-diff", files = function()
				local f = table.clone(file)
				f.rev = if revision:get() then "short" else "long"
				return { f }
			end, narrow = true, bare = mode == "bare",
			open = opened, wrap = wrap, split = split,
			lines = function() return if revision:get() then short else long end,
			after = function(_path, key)
				if key ~= git.line_key(long[2]) then return nil end
				return { { kind = "comment", build = function()
					return ui.div { id = "pinned-comment", w_full = true, h = 40, ui.text { "A comment stays in place" } }
				end } }
			end,
		}
	end, { grow = 1, min_h = 0, w_full = true }),
}
