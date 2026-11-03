-- A number stepper: a number in the field's frame between a − and a +.
-- Type a number (it applies on Enter or on leaving; anything else goes
-- back), press the buttons, or use the arrows (ten steps with Shift). At
-- an end its button dims. A unit may follow the number ("px", "s").
--
-- `NumberStepper { value = state, min = 1, max = 16, step = 1, unit = "agents" }`.

local Icon = require("@self/icon")
local control = require("@self/control")
local field = require("@self/field")
local locale = require("@self/locale")
local EditorMenu = require("@self/editor_menu")

export type Props = {
	value: State<number>,
	min: number?,
	max: number?,
	step: number?,
	decimals: number?,
	unit: (string | Message)?,
	-- (These take messages, `ui.t`, too.)
	label: (string | Message)?,
	hint: (string | Message)?,
	name: (string | Message)?,
	size: control.Size?,
	disabled: boolean?,
	on_change: ((v: number) -> ())?,
	w: number?,
}

return ui.component(function(props: Props): Node
	local size: control.Size = props.size or "md"
	local value = props.value
	local lo, hi = props.min or -math.huge, props.max or math.huge
	local step = props.step or 1
	local decimals = props.decimals or 0
	local focused = ui.use_state(false)
	local hovered = ui.use_state(false)

	local function fmt(v: number): string
		return string.format(`%.{decimals}f`, v)
	end
	local function set(v: number)
		local m = 10 ^ decimals
		v = math.clamp(math.floor(v * m + 0.5) / m, lo, hi)
		if v ~= value:peek() then
			value:set(v)
			if props.on_change then
				props.on_change(v)
			end
		end
	end

	local function stepper(icon: Icon.Name, label: string | Message, by: number): Node
		local at_end = function(): boolean
			local v = value:get()
			return if by < 0 then v <= lo else v >= hi
		end
		local box = control.H[size] - 6
		return ui.button {
			label = label,
			w = box,
			h = box,
			px = 0,
			py = 0,
			shrink = 0,
			justify = "center",
			rounded = ui.raw(control.nest(control.RADIUS[size], (control.H[size] - box) / 2), "concentric with the field: its corner less the room round the button"),
			bg = "#00000000",
			color = "$text.secondary",
			cursor = "pointer",
			-- Not a Tab stop: the field's arrows do what these do.
			focusable = false,
			hover = { bg = "$hover", color = "$text.primary" },
			active = { bg = "$pressed", color = "$text.primary" },
			disabled = at_end,
			opacity = function()
				return if at_end() then 0.35 else 1
			end,
			on_click = function()
				set(value:peek() + by)
			end,
			Icon { name = icon, size = control.ICON[size] - 2 },
		}
	end

	local input_props = field.input({
		value = function()
			return fmt(value:get())
		end,
		label = props.label or props.name or "",
		inputmode = "decimal",
		select_on_focus = true,
		align = "center",
		on_commit = function(text: string)
			local n = tonumber(text)
			if n then
				set(n)
			else
				-- Not a number: back to what it was.
				value:set(value:peek())
			end
		end,
		on_key = function(e)
			local big = if e.shift then 10 else 1
			if e.key == "ArrowUp" then
				set(value:peek() + step * big)
				return true
			elseif e.key == "ArrowDown" then
				set(value:peek() - step * big)
				return true
			elseif e.key == "PageUp" then
				set(value:peek() + step * 10)
				return true
			elseif e.key == "PageDown" then
				set(value:peek() - step * 10)
				return true
			end
			return false
		end,
	}, size, focused)
	input_props.tabular = true
	input_props.role = "spinbutton"
	local input = ui.input(EditorMenu.on(input_props))

	local box = field.frame {
		size = size,
		focused = focused,
		hovered = hovered,
		px = "$zeron.space.hair",
		gap = "$zeron.space.hair",
		w_full = true,
		stepper("minus", locale.t("ds.stepper.decrease"), -step),
		input,
		if props.unit
			then ui.text { props.unit, shrink = 0, text_size = control.TEXT[size], color = "$text.tertiary", pr = "$space.1" }
			else nil,
		stepper("plus", locale.t("ds.stepper.increase"), step),
	}
	return field.wrap {
		label = props.label,
		hint = props.hint,
		size = size,
		box = box,
		disabled = props.disabled,
		w = props.w or (if props.unit then 140 else 112),
	}
end)
