-- A checkbox: a small rounded box, a quiet well while off; on, it fills
-- with the accent and its check springs in; mixed (some of what it stands
-- for is on: a folder of files partly staged), a dash in the same fill.
-- Beside it its label, and a line under the label saying more. The whole
-- row takes the click; hover firms the box's edge.
--
-- `Checkbox { value = state, label = "Stage all" }`. Controlled, as zgx's
-- controls are: `value` a function and `on_change` hearing the click (a
-- folder's box, worked out from its files). `mixed` (a function or state)
-- shows the dash while true; clicking a mixed box turns it on.

local motion = require("@self/motion")
local control = require("@self/control")

export type Props = {
	value: State<boolean> | () -> boolean,
	mixed: (State<boolean> | () -> boolean)?,
	on_change: ((on: boolean) -> ())?,
	-- (Its words may be messages, `ui.t`.)
	label: (string | Message | () -> string)?,
	description: (string | Message)?,
	disabled: boolean?,
	-- Its accessible name when it has no label.
	name: Label?,
	-- The box alone, no padding round it (a list row's, which takes the
	-- click itself).
	bare: boolean?,
}

local BOX = 16

-- The box, drawn from whether it is on and mixed, and whether the row is
-- under the pointer. Shared with the radio's look.
local function box(on: () -> boolean, mixed: () -> boolean, hovered: () -> boolean): Node
	local filled = ui.computed(function(): boolean
		return on() or mixed()
	end)
	return ui.div {
		w = BOX,
		h = BOX,
		shrink = 0,
		relative = true,
		rounded = "$radius.sm",
		border = 1,
		bg = function()
			return if filled:get() then "$zeron.check.on" else "$zeron.check.bg"
		end,
		border_color = function()
			if filled:get() then
				return "$zeron.check.on"
			end
			return if hovered() then "$text.tertiary" else "$zeron.check.edge"
		end,
		color = "$zeron.check.mark",
		items = "center",
		justify = "center",
		-- The check: springing in from a little smaller as it turns on.
		ui.div {
			absolute = true,
			left = 0,
			top = 0,
			w = BOX - 2,
			h = BOX - 2,
			items = "center",
			justify = "center",
			opacity = ui.tween(ui.computed(function(): number
				return if on() and not mixed() then 1 else 0
			end), { duration = 120 }),
			scale = motion.spring(ui.computed(function(): number
				return if on() and not mixed() then 1 else 0.5
			end), { stiffness = 520, damping = 26 }),
			ui.svg {
				-- (As code, as ds/icon draws: a plugin's copy draws it too.)
				src = `code:{require("@ds/icon_svgs").check}`,
				w = 12,
				h = 12,
				tint = "currentColor",
			},
		},
		-- The dash.
		ui.div {
			absolute = true,
			left = 3,
			top = 6,
			w = 8,
			h = 2,
			rounded = "$radius.full",
			bg = "$zeron.check.mark",
			opacity = ui.tween(ui.computed(function(): number
				return if mixed() then 1 else 0
			end), { duration = 120 }),
			scale = motion.spring(ui.computed(function(): number
				return if mixed() then 1 else 0.4
			end), { stiffness = 520, damping = 26 }),
		},
	}
end

local function reader(v: any): () -> boolean
	if v == nil then
		return function()
			return false
		end
	elseif type(v) == "function" then
		return v
	end
	return function()
		return v:get()
	end
end

local Checkbox = ui.component(function(props: Props): Node
	local hovered = ui.use_state(false)
	local on = reader(props.value)
	local mixed = reader(props.mixed)
	local function flip()
		if props.disabled then
			return
		end
		local next = if mixed() then true else not on()
		local v = props.value
		if type(v) ~= "function" then
			(v :: State<boolean>):set(next)
		end
		if props.on_change then
			props.on_change(next)
		end
	end
	local label = props.label
	local text: Node? = nil
	if label then
		text = ui.column {
			gap = "$zeron.space.hair",
			min_w = 0,
			grow = 1,
			ui.text {
				label,
				text_size = "$font.size.md",
				line_height = "$font.line.md",
				color = "$text.primary",
				wrap = true,
			},
			if props.description
				then ui.text {
					props.description,
					text_size = "$font.size.sm",
					line_height = "$font.line.sm",
					color = "$text.tertiary",
					wrap = true,
				}
				else nil,
		}
	end
	return ui.row {
		role = "checkbox",
		label = props.name or label,
		checked = on,
		focusable = not props.disabled,
		disabled = props.disabled,
		opacity = if props.disabled then "$zeron.disabled" else nil,
		cursor = if props.disabled then nil else "pointer",
		gap = "$space.2",
		-- The box sits on the label's first line (18 high at md).
		items = "start",
		rounded = "$radius.sm",
		focus = control.ring,
		on_hover = function(inside: boolean)
			hovered:set(inside)
		end,
		on_click = flip,
		on_key = function(e)
			if e.key == " " then
				flip()
				return true
			end
			return false
		end,
		ui.div {
			-- (The label's line is 18: the box centred on it.)
			pt = if text then ui.raw(1, "centres the 16 box on an 18 line") else 0,
			shrink = 0,
			box(on, mixed, function()
				return hovered:get()
			end),
		},
		text,
	}
end)

return Checkbox
