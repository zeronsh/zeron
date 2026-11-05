-- Buttons, as Zeron's are: 8-point corners (6 small) unless `shape =
-- "pill"`. `Button { "Continue", on_click = … }` is the primary; `tone`
-- picks its weight (`danger` for a confirm that can't be taken back),
-- `size` its height (`xs`, a header's or a bar's, the panel controls' 24). An icon goes beside the text as a
-- child (`ds.Icon`); `icon = true` makes a square button of an icon alone
-- (give it a `label`). Every tone says its own hover and press: zgx's button
-- would otherwise lay its own pressed wash over the fill.

local recipe = ui.variants {
	base = {
		items = "center",
		justify = "center",
		gap = "$space.2",
		font = "$font.sans",
		weight = "$font.weight.medium",
		cursor = "pointer",
		disabled = { opacity = 0.4 },
	},
	variants = {
		tone = {
			-- The way on: one per screen.
			primary = {
				bg = "$zeron.primary.bg",
				color = "$zeron.primary.text",
				hover = { bg = "$zeron.primary.hover" },
				active = { bg = "$zeron.primary.active" },
			},
			-- Beside it: a fill, firmer as it is touched.
			secondary = {
				bg = "$zeron.secondary.bg",
				color = "$text.primary",
				hover = { bg = "$zeron.secondary.hover" },
				active = { bg = "$zeron.secondary.active" },
			},
			-- Back, Skip: text until touched.
			ghost = {
				bg = "#00000000",
				color = "$text.secondary",
				hover = { bg = "$zeron.secondary.bg", color = "$text.primary" },
				active = { bg = "$zeron.secondary.hover", color = "$text.primary" },
			},
			accent = {
				bg = "$accent",
				color = "$accent.text",
				hover = { bg = "$accent.hover" },
				active = { bg = "$zeron.accent.active" },
			},
			-- Delete, Discard: a confirm that can't be taken back.
			danger = {
				bg = "$zeron.destructive.bg",
				color = "$zeron.destructive.text",
				hover = { bg = "$zeron.destructive.hover" },
				active = { bg = "$zeron.destructive.active" },
			},
		},
		size = {
			-- A header's or a bar's: the panel's controls' height (its icon
			-- buttons', its chips'), their padding and corner.
			xs = { h = 24, px = "$zeron.space.tight", rounded = "$radius.lg", text_size = "$font.size.sm" },
			sm = { h = 28, px = "$zeron.space.snug", rounded = "$radius.md", text_size = "$font.size.sm" },
			md = { h = "$control.height", px = "$space.3", rounded = "$radius.lg", text_size = "$font.size.md" },
			lg = { h = 40, px = "$space.5", rounded = "$radius.lg", text_size = "$font.size.lg" },
		},
		shape = { rounded = {}, pill = {} },
		icon = { [true] = {}, [false] = {} },
	},
	-- Shape and icon decide over the sizes' corners and padding.
	compound = {
		{ shape = "pill", rounded = "$radius.full" },
		{ icon = true, size = "xs", w = 24, px = 0 },
		{ icon = true, size = "sm", w = 28, px = 0 },
		{ icon = true, size = "md", w = "$control.height", px = 0 },
		{ icon = true, size = "lg", w = 40, px = 0 },
	},
	defaults = { tone = "primary", size = "md", shape = "rounded", icon = false },
}

export type Tone = "primary" | "secondary" | "ghost" | "accent" | "danger"
export type Size = "xs" | "sm" | "md" | "lg"
export type Shape = "rounded" | "pill"
export type Props = Merge<ContainerProps, {
	tone: Tone?,
	size: Size?,
	shape: Shape?,
	icon: boolean?,
	-- Working (a state or a function): a spinner in the label's place, the
	-- label kept (unseen) so the button holds its width, and clicks ignored.
	-- Never "Saving…".
	busy: (State<boolean> | () -> boolean)?,
	[number]: Child,
}>

-- The spinner in each tone's text colour, and at each size.
local INK: { [string]: string } = {
	primary = "$zeron.primary.text",
	secondary = "$text.primary",
	ghost = "$text.secondary",
	accent = "$accent.text",
	danger = "$zeron.destructive.text",
}
local SPIN: { [string]: number } = { sm = 12, md = 14, lg = 16 }

local status = require("@self/status")

return function(props: Props): Node
	local p = table.clone(props) :: any
	p.variants = recipe { tone = props.tone, size = props.size, shape = props.shape, icon = props.icon }
	p.tone, p.size, p.shape, p.icon, p.busy = nil, nil, nil, nil, nil
	local given = props.busy
	if given == nil then
		return ui.button(p)
	end
	local function busy(): boolean
		return if type(given) == "function" then given() == true else (given :: any):get() == true
	end
	-- The label, unseen while busy: it keeps the button's width.
	local label: { [any]: any } = {
		items = "center",
		gap = "$space.2",
		opacity = function(): number
			return if busy() then 0 else 1
		end,
	}
	for i, child in ipairs(props :: any) do
		label[i] = child
		p[i] = nil
	end
	p.relative = true
	p[1] = ui.row(label)
	-- (Its box always there, out of the flow, the spinner shown in it: a
	-- shown-or-not beside the label counted a gap in the button's width as
	-- what it's in measured it, 8 points past its box.)
	p[2] = ui.row {
		absolute = true,
		left = 0,
		right = 0,
		top = 0,
		bottom = 0,
		items = "center",
		justify = "center",
		pointer_events = "none",
		ui.show(busy, status.Spinner { size = SPIN[props.size or "md"], ink = INK[props.tone or "primary"] }),
	}
	local click = props.on_click
	if click then
		p.on_click = function(...)
			if not busy() then
				click(...)
			end
		end
	end
	return ui.button(p)
end
