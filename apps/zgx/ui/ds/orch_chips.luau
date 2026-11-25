-- Chips to pick several of: a pill each, a dot and the selection's wash
-- on the ones chosen (where an orchestrator's crew runs: one per device).
-- A click flips one; each is a checkbox to assistive technology, the row
-- a group named by its label.
--
--   Chips { chosen = function() return set end, on_change = function(set) … end,
--     options = function() return { { value, label } } end,
--     label = "Crew runs on", ids = "orch-new-devices" }
--
-- `ids` names the row, and each chip `<ids>-<value>`.
--
-- `chosen` says which show chosen, as a set (`{ [value] = true }`): the
-- caller's to decide what none means. A click hands `on_change` the set
-- with that one flipped.

local control = require("@self/control")
local field = require("@self/field")

export type Option = { value: string, label: string }
export type Props = {
	chosen: () -> { [string]: boolean },
	on_change: (chosen: { [string]: boolean }) -> (),
	options: () -> { Option },
	label: (string | Message)?,
	ids: string?,
}

local SIZE = "sm"
local DOT = 7

local function chip(props: Props, o: Option): Node
	local function on(): boolean
		return props.chosen()[o.value] == true
	end
	return ui.button {
		id = if props.ids then `{props.ids}-{o.value}` else nil,
		role = "checkbox",
		label = o.label,
		checked = on,
		h = control.H[SIZE],
		px = "$space.3",
		gap = "$space.2",
		items = "center",
		shrink = 0,
		rounded = "$radius.full",
		border = 1,
		border_color = function(): string
			return if on() then "$zeron.accented.rail" else "$border.subtle"
		end,
		bg = function(): string
			return if on() then "$zeron.selected.flat.bg" else "#00000000"
		end,
		color = function(): string
			return if on() then "$text.primary" else "$text.secondary"
		end,
		cursor = "pointer",
		hover = { bg = "$hover" },
		active = { bg = "$pressed" },
		focus = control.ring,
		on_click = function()
			local now = table.clone(props.chosen())
			now[o.value] = if now[o.value] then nil else true
			props.on_change(now)
		end,
		ui.show(on, ui.div {
			w = DOT,
			h = DOT,
			shrink = 0,
			rounded = "$radius.full",
			bg = "$accent",
			aria_hidden = true,
		}),
		ui.text { o.label, text_size = control.TEXT[SIZE], literal = true },
	}
end

return ui.component(function(props: Props): Node
	-- (Labelled as a field is: ds/field's label over it.)
	return ui.column {
		w_full = true,
		gap = "$zeron.space.tight",
		if props.label then field.label(props.label, SIZE) else nil,
		ui.row {
			id = props.ids,
			role = "group",
			label = props.label,
			w_full = true,
			flex_wrap = true,
			gap = "$space.2",
			ui.each(ui.computed(props.options), function(o: Option): Node
				return chip(props, o)
			end, { key = function(o: Option): string
				return o.value
			end }),
		},
	}
end)
