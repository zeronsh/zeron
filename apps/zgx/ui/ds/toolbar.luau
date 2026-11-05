-- A toolbar: a row of controls over what they act on (a diff's view mode,
-- wrap and whitespace; an editor's find). One Tab stop, the arrows moving
-- between its controls, as a native toolbar's do. Groups sit apart with a
-- hairline between; `Toolbar.Spacer` pushes what follows to the far end.
-- Its controls are the design system's own: IconButton, ToggleButton,
-- Segmented, Tabs (pill), Button.
--
--   Toolbar { label = "Diff",
--     Toolbar.Group { Segmented { … } },
--     Toolbar.Group { ToggleButton { icon = "wrap-text", … }, ToggleButton { … } },
--     Toolbar.Spacer {},
--     Button { "Open file", size = "sm", tone = "ghost" } }
--
-- `look = "bar"` sets it on a strip of its own (a panel's header: a
-- hairline under it); `look = "island"` floats it on the island's glass.

local glass = require("@self/glass")
local control = require("@self/control")

local M = {}

export type Props = {
	label: Label?,
	look: ("plain" | "bar" | "island")?,
	[number]: Child,
}

local function children(props: { [any]: any }): { any }
	local out: { any } = {}
	for _, c in ipairs(props) do
		table.insert(out, c)
	end
	return out
end

function M.Toolbar(props: Props): Node
	local look = props.look or "plain"
	local kids = children(props :: any)
	-- A hairline between groups.
	local items: { any } = {}
	local last_group = false
	for _, c in kids do
		local is_group = type(c) == "table" and (c :: any).__toolbar_group
		if is_group and last_group then
			table.insert(items, ui.div { w = 1, h = 16, shrink = 0, mx = "$space.1", bg = "$border", aria_hidden = true })
		end
		table.insert(items, if is_group then (c :: any).node else c)
		last_group = is_group
	end
	local p: { [any]: any } = {
		role = "toolbar",
		label = props.label,
		gap = "$space.1",
		items = "center",
		table.unpack(items),
	}
	if look == "bar" then
		p.w_full = true
		p.h = 40
		p.px = "$space.2"
		p.border_sides = { 0, 0, 1, 0 }
		p.border_color = "$border.subtle"
	elseif look == "island" then
		p.shrink = 0
		glass.style(p, "island")
		control.contain(p, { radius = "$zeron.island.radius", inset = "$space.1", border = 1 })
	end
	return ui.row(p)
end

-- A group of controls, kept together, a hairline from its neighbours.
function M.Group(props: { [number]: Child }): any
	return {
		__toolbar_group = true,
		node = ui.row { gap = "$zeron.space.hair", items = "center", shrink = 0, table.unpack(children(props :: any)) },
	}
end

-- Pushes what follows to the far end.
function M.Spacer(_props: { [any]: any }?): Node
	return ui.div { grow = 1 }
end

return M
