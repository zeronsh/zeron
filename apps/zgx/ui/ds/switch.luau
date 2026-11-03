-- A switch, flat and glassy: a translucent track (the glass shows through
-- it; on, the accent's) and a plain white knob that springs across.
-- `Switch { value = state, label = "Frosted" }`.

export type Props = {
	value: State<boolean>,
	-- What assistive technology calls it.
	label: Label?,
	on_change: ((on: boolean) -> ())?,
}

local motion = require("@self/motion")

local W, H, KNOB, INSET = 40, 24, 18, 3

return function(props: Props): Node
	local value = props.value
	local function flip()
		value:toggle()
		if props.on_change then
			props.on_change(value:peek())
		end
	end
	return ui.div {
		role = "switch",
		checked = value,
		label = props.label,
		focusable = true,
		cursor = "pointer",
		-- Focused from the keys, zgx's control ring (a point of the focus
		-- colour), just outside the track: inside, the accent track would
		-- swallow it while on.
		focus = { outline = 1, outline_offset = 1, outline_color = "$focus.ring" },
		on_click = flip,
		on_key = function(e)
			if e.key == " " or e.key == "Enter" then
				flip()
				return true
			end
			return false
		end,
		w = W,
		h = H,
		shrink = 0,
		-- The knob's box: it is placed in the track, whatever is around it.
		relative = true,
		rounded = "$radius.full",
		bg = ui.tween(value, { from = "$zeron.switch.track", to = "$zeron.switch.on", duration = 180 }),
		ui.div {
			absolute = true,
			left = INSET,
			top = INSET,
			w = KNOB,
			h = KNOB,
			rounded = "$radius.full",
			bg = "$zeron.switch.knob",
			shadow = "$zeron.switch.knob_lift",
			x = motion.spring(value, { from = 0, to = W - KNOB - INSET * 2, stiffness = 420, damping = 30 }),
		},
	}
end
