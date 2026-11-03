-- The gallery's Glass page: a menu's panel and an island's bar, over
-- something to blur, frosted or opaque as the header says.

local ds = require("@ds")

local function block(title: string, ...: Child): Node
	return ui.column { gap = "$space.3", ds.SectionTitle { title }, ... }
end

-- Something under the glass, so its blur shows.
local function backdrop(...: Child): Node
	local stripes: { Node } = {}
	for i, hue in { "violet", "blue", "emerald", "amber", "rose" } do
		table.insert(stripes, ui.div {
			absolute = true,
			left = (i - 1) * 72,
			top = 0,
			w = 36,
			h = 160,
			bg = `$palette.{hue}.400`,
		})
	end
	-- (Appended, not unpacked mid-table: Luau would keep only the first.)
	local box: { [any]: any } = { w = 360, h = 160, rounded = "$radius.lg", overflow = "hidden" }
	for _, stripe in stripes do
		table.insert(box, stripe)
	end
	table.insert(box, ui.row { w_full = true, h_full = true, items = "center", justify = "center", ... })
	return ui.overlay(box)
end

return {
	id = "glass",
	title = "Glass",
	about = "What floats over the page: a menu's panel (menus, dropdowns, flyouts, a dragged card) and an island's bar (a find bar over text). Opaque, the menu turns solid.",
	demo = function(): Node
		return ui.column {
			gap = "$space.6",
			block(
				"MENU",
				backdrop(ds.Glass {
					kind = "menu",
					w = 200,
					p = "$space.3",
					ds.Text { "A menu's panel" },
					ds.Hint { "Tint, blur, hairline, deep shadow." },
				})
			),
			block(
				"ISLAND",
				backdrop(ds.Glass {
					kind = "island",
					p = "$space.1",
					ui.row {
						gap = "$zeron.space.hair",
						ds.IconButton { icon = "arrow-up", label = "Previous", size = "sm" },
						ds.IconButton { icon = "arrow-down", label = "Next", size = "sm" },
						ds.IconButton { icon = "close", label = "Close", size = "sm" },
					},
				})
			),
		}
	end,
}
