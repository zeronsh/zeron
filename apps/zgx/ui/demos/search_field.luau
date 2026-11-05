-- Search field.

local SearchField = require("@ds/search_field")
local kit = require("@demos/kit")

local query = ui.state("sf_query", "")
local branches = ui.state("sf_branches", "zeron/")
local pill = ui.state("sf_pill", "")
local small = ui.state("sf_small", "")

local BRANCHES = { "main", "zeron/primitives", "zeron/composer", "zeron/git-pages", "release/0.3" }

return {
	id = "search_field",
	title = "Search field",
	about = "A text field with the glass before it: Escape or × clears it. Empty, it can show the keys that reach it; typed into, how many results it found.",
	demo = function()
		return kit.page(
			kit.block("EMPTY, WITH ITS KEYS", SearchField { value = query, placeholder = "Search settings", shortcut = "cmd+f", w = 280 }),
			kit.block(
				"FILTERING, WITH A COUNT",
				SearchField {
					value = branches,
					placeholder = "Filter branches",
					w = 280,
					results = function()
						local q = string.lower(branches:get())
						local n = 0
						for _, b in BRANCHES do
							if string.find(b, q, 1, true) then
								n += 1
							end
						end
						return n
					end,
				}
			),
			kit.block(
				"SHAPES AND SIZES",
				kit.row(
					SearchField { value = small, size = "sm", placeholder = "Search files", w = 200 },
					SearchField { value = pill, shape = "pill", placeholder = "Search pull requests", w = 260 }
				)
			)
		)
	end,
}
