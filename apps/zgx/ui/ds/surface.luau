-- Surfaces: what content sits in when it doesn't sit straight on the glass.
-- `Surface { kind = "raised", … }`. Depth is the theme's to say (lighter and
-- lit in dark, whiter and lined in light): the kinds only name it through
-- tokens. Selection (`selected = true`, or a state or function choosing it)
-- is its own fill, ring, highlight and lift; hover is a wash.

local recipe = ui.variants {
	base = {
		rounded = "$radius.lg",
		border = 1,
		border_color = "#00000000",
	},
	variants = {
		kind = {
			-- Flat on the window's glass.
			plain = {},
			raised = {
				bg = "$surface.raised",
				border_color = "$zeron.edge",
				inset_shadow = "$zeron.highlight",
				shadow = "$zeron.lift",
			},
			sunken = {
				bg = "$surface.sunken",
				border_color = "$zeron.well.edge",
				inset_shadow = "$zeron.well.inset",
			},
			-- Floating over other content: popovers, sheets.
			overlay = {
				bg = "$surface.overlay",
				border_color = "$border",
				inset_shadow = "$zeron.highlight",
				shadow = "$shadow.lg",
				backdrop_blur = 30,
			},
		},
		interactive = {
			[true] = { cursor = "pointer", hover = { bg = "$hover" }, active = { bg = "$pressed" } },
			[false] = {},
		},
		selected = { [true] = {}, [false] = {} },
		-- How a selection stands: raised (lit, lifted, ringed) for cards and
		-- options, flat (the wash alone) for rows in a sidebar or list.
		depth = { raised = {}, flat = {} },
	},
	-- Selected wins over any kind and over hover and press (a wash under the
	-- selected fill would dim it in light).
	compound = {
		{
			selected = true,
			depth = "raised",
			bg = "$zeron.selected.bg",
			border_color = "$zeron.selected.ring",
			inset_shadow = "$zeron.selected.highlight",
			shadow = "$zeron.selected.lift",
			hover = { bg = "$zeron.selected.bg" },
			active = { bg = "$zeron.selected.bg" },
		},
		{
			selected = true,
			depth = "flat",
			bg = "$zeron.selected.flat.bg",
			hover = { bg = "$zeron.selected.flat.bg" },
			active = { bg = "$zeron.selected.flat.bg" },
		},
	},
	defaults = { kind = "plain", interactive = false, selected = false, depth = "raised" },
}

export type Kind = "plain" | "raised" | "sunken" | "overlay"
export type Depth = "raised" | "flat"
export type Props = Merge<ContainerProps, {
	kind: Kind?,
	depth: Depth?,
	interactive: boolean?,
	selected: (boolean | State<boolean> | () -> boolean)?,
	[number]: Child,
}>

return function(props: Props): Node
	local p = table.clone(props) :: any
	p.variants = recipe {
		kind = props.kind,
		depth = props.depth,
		interactive = props.interactive or props.on_click ~= nil,
		selected = props.selected :: any,
	}
	p.kind, p.depth, p.interactive, p.selected = nil, nil, nil, nil
	return ui.column(p)
end
