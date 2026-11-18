-- A list scrolled by the view around it (`virtualize = "ancestor"`),
-- built as a `ui.presence` swaps to it with an opacity enter. It builds
-- no rows: none until the view scrolls or the window resizes. Without
-- the enter (or with `mode` and `exit` alone), it builds the rows in view
-- at once. Seen on zgx b80a8af1.
local fade = ui.state("fade", true)
local shown = ui.state("shown", "empty")
local rows = {}
for i = 1, 40 do
	table.insert(rows, { id = `r{i}` })
end
local data = ui.state("rows", rows)
ui.command { id = "show", title = "Show the list", run = function() shown:set("list") end }
ui.command { id = "no_fade", title = "No enter", run = function() fade:set(false) end }
local function list(): Node
	return ui.list {
		data = data,
		key = function(r: any): string
			return r.id
		end,
		heights = function(): { number }
			local out = {}
			for i = 1, #data:get() do
				out[i] = 40
			end
			return out
		end,
		w_full = true,
		virtualize = "ancestor",
		row = function(r: any): Node
			return ui.button { r.id, w_full = true, h = 40 }
		end,
	}
end
return ui.scroll {
	w = 800,
	h = 600,
	ui.column {
		w_full = true,
		ui.text { "Above the list" },
		ui.presence(function(): string
			return shown:get() .. (if fade:get() then ":fade" else "")
		end, function(key: string): Node
			if string.match(key, "^empty") then
				return ui.text { "Nothing yet" }
			end
			return list()
		end, if fade:peek() then { w_full = true, enter = { opacity = { 0, 1 }, duration = 120 } } else { w_full = true }),
	},
}
