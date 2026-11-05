-- ui/hit_fixture.luau (the main window with a transcript) with the bug the
-- hit sweep (src/hit_sweep_tests.rs) is for, on command: an invisible layer
-- over the whole window that takes the pointer ("hit.cover"), or one that
-- lets it through ("hit.cover_through"); "hit.uncover" takes it away.

local node = require("./hit_fixture")

-- The bug the sweep is for, on command: an invisible layer over the page
-- that takes the pointer ("cover"), or lets it through ("cover_through").
local cover = ui.state("hit_cover", "none")
for _, kind in { "cover", "cover_through", "uncover" } do
	ui.command {
		id = `hit.{kind}`,
		title = `Hit sweep: {kind}`,
		run = function()
			cover:set(if kind == "uncover" then "none" else kind)
		end,
	}
end

-- (Wrapped to lay the cover over it: the layout shifts a little, which
-- the cover's test doesn't mind; the sweep's own fixture isn't wrapped.)
-- A layer over the whole window, as the bug's would be: drawn nearly clear.
local function layer(id: string, props: { [string]: any }): Node
	local node: { [any]: any } = {
		id = id,
		absolute = true,
		left = 0,
		right = 0,
		top = 0,
		bottom = 0,
		opacity = 0.02,
		bg = "$text.primary",
	}
	for k, v in props do
		node[k] = v
	end
	return ui.div(node)
end

-- (Wrapped to lay the cover over it: the layout shifts a little, which
-- the cover's test doesn't mind; the sweep's own fixture isn't wrapped.)
return ui.div {
	w_full = true,
	h_full = true,
	relative = true,
	node,
	-- Taking the pointer, as a layer that listens does.
	ui.show(function(): boolean
		return cover:get() == "cover"
	end, layer("hit-cover", { on_click = function() end })),
	-- Letting it through: nothing of it listens.
	ui.show(function(): boolean
		return cover:get() == "cover_through"
	end, layer("hit-cover-through", { pointer_events = "none" })),
}
