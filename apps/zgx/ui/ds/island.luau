-- An island's look and measure (the theme's `zeron.island`), for every
-- island: the title bar's controls over the page, the sidebar's rail. One
-- definition, so the two are the same thing turned: the rail as wide as the
-- title bar's island is tall, its controls the same size, as far apart, as
-- far from the window's edge.
--
-- `island.glass { … }`: a node's props with the island's glass added.
-- `island.metrics()`: its numbers (read as it's built).

local material = require("@self/material")

local M = {}

export type Metrics = { radius: number, inset: number, pad: number, control: number, gap: number, group: number, blur: number }

local function n(token: string, fallback: number): number
	return (ui.theme.get(token) :: number?) or fallback
end

function M.metrics(): Metrics
	return {
		radius = n("zeron.island.radius", 12),
		inset = n("zeron.island.inset", 6),
		pad = n("zeron.island.pad", 4),
		control = n("zeron.island.control", 24),
		gap = n("zeron.island.gap", 2),
		group = n("zeron.island.group", 8),
		blur = n("zeron.island.blur", 20),
	}
end

-- The island's thickness: a control and its padding either side (the title
-- bar's island's height, the rail's width).
function M.thickness(): number
	local m = M.metrics()
	return m.control + m.pad * 2
end

-- Its glass, or with glass turned solid (ds/material: the Opaque setting,
-- or the system's transparency off), the menus' solid fill, unblurred.
function M.glass(props: { [any]: any }): { [any]: any }
	props.rounded = "$zeron.island.radius"
	props.bg = function()
		return if material.see_through() then "$zeron.island.tint" else "$zeron.menu.solid"
	end
	props.backdrop_blur = function()
		return if material.see_through() then "$zeron.island.blur" else 0
	end
	props.shadow = "$zeron.island.shadow"
	return props
end

return M
