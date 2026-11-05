-- A radio group: one of a few options, each a round well beside its label
-- (a line under it saying more), the chosen one filled with the accent
-- round a dot that springs in. One Tab stop for the group; the arrows move
-- the choice, as a native group's do.
--
-- `RadioGroup { value = state, options = { { value = "merge", label = "Merge commit" }, … } }`.
-- `direction = "row"` lays them across (a few short ones).

local control = require("@self/control")

-- (An option's words may be messages, `ui.t`, drawn as they are.)
export type Option = { value: any, label: string | Message, description: (string | Message)?, disabled: boolean? }
export type Props = {
	value: State<any>,
	options: { Option },
	direction: ("column" | "row")?,
	-- What assistive technology calls the group.
	label: Label?,
	disabled: boolean?,
	on_change: ((value: any) -> ())?,
}

local DOT = 16

local Radio = ui.component(function(props: { option: Option, value: State<any>, on_choose: (v: any) -> () }): Node
	local option = props.option
	local hovered = ui.use_state(false)
	local function chosen(): boolean
		return props.value:get() == option.value
	end
	return ui.row {
		role = "radio",
		checked = chosen,
		label = option.label,
		focusable = not option.disabled,
		disabled = option.disabled,
		opacity = if option.disabled then "$zeron.disabled" else nil,
		cursor = if option.disabled then nil else "pointer",
		gap = "$space.2",
		items = "start",
		rounded = "$radius.sm",
		focus = control.ring,
		on_hover = function(inside: boolean)
			hovered:set(inside)
		end,
		on_click = function()
			props.on_choose(option.value)
		end,
		on_focus = function()
			-- Arrowing onto an option chooses it, as a native group does.
			if not chosen() then
				props.on_choose(option.value)
			end
		end,
		ui.div {
			pt = ui.raw(1, "centres the 16 dot on an 18 line"),
			shrink = 0,
			ui.div {
				w = DOT,
				h = DOT,
				rounded = "$radius.full",
				border = 1,
				items = "center",
				justify = "center",
				bg = function()
					return if chosen() then "$zeron.check.on" else "$zeron.check.bg"
				end,
				border_color = function()
					if chosen() then
						return "$zeron.check.on"
					end
					return if hovered:get() then "$text.tertiary" else "$zeron.check.edge"
				end,
				ui.div {
					w = 6,
					h = 6,
					rounded = "$radius.full",
					bg = "$zeron.check.mark",
					scale = ui.spring(ui.computed(function(): number
						return if chosen() then 1 else 0
					end), { stiffness = 520, damping = 26 }),
				},
			},
		},
		ui.column {
			gap = "$zeron.space.hair",
			min_w = 0,
			grow = 1,
			ui.text {
				option.label,
				text_size = "$font.size.md",
				line_height = "$font.line.md",
				color = "$text.primary",
				wrap = true,
			},
			if option.description
				then ui.text {
					option.description,
					text_size = "$font.size.sm",
					line_height = "$font.line.sm",
					color = "$text.tertiary",
					wrap = true,
				}
				else nil,
		},
	}
end)

return function(props: Props): Node
	local function choose(v: any)
		if props.value:peek() == v then
			return
		end
		props.value:set(v)
		if props.on_change then
			props.on_change(v)
		end
	end
	local items: { Node } = {}
	for _, option in props.options do
		table.insert(items, Radio { option = option, value = props.value, on_choose = choose })
	end
	local row = props.direction == "row"
	return ui.column {
		role = "radiogroup",
		label = props.label,
		focus_group = if row then "horizontal" else "vertical",
		direction = if row then "row" else "column",
		gap = if row then "$space.5" else "$space.3",
		disabled = props.disabled,
		opacity = if props.disabled then "$zeron.disabled" else nil,
		table.unpack(items),
	}
end
