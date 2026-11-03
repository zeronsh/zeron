-- A slider: a number in a range, dragged along a groove. The groove is the
-- switch's (translucent, the glass showing through), filled with the
-- accent up to a white knob that is the switch's knob, so the two read as
-- one family. The knob swells a little under the pointer and more while
-- held; the arrows step it (ten steps with Shift), Home and End go to its
-- ends. Marks along the groove, if given, are where a drag settles.
--
-- `Slider { value = state, min = 0, max = 100, step = 5, label = "Volume" }`;
-- `format` shows the value at the end (`function(v) return `{v}%` end`).

local control = require("@self/control")
local locale = require("@self/locale")

export type Props = {
	value: State<number>,
	min: number?,
	max: number?,
	step: number?,
	-- Where a drag settles, and what each says under the groove.
	marks: { { value: number, label: (string | Message)? } }?,
	-- What assistive technology calls it (a message too).
	label: (string | Message)?,
	format: ((v: number) -> string)?,
	disabled: boolean?,
	on_change: ((v: number) -> ())?,
	w: number?,
	w_full: boolean?,
	grow: number?,
}

local TRACK, KNOB = 4, 16

return ui.component(function(props: Props): Node
	local value = props.value
	local lo, hi = props.min or 0, props.max or 1
	local step = props.step
	local hovered = ui.use_state(false)
	local held = ui.use_state(false)
	local focused = ui.use_state(false)
	local track = ui.ref()

	local function fraction(): number
		return math.clamp((value:get() - lo) / (hi - lo), 0, 1)
	end
	local function snap(v: number): number
		v = math.clamp(v, lo, hi)
		local marks = props.marks
		if marks and #marks > 0 and not step then
			local best, dist = v, math.huge
			for _, m in marks do
				local d = math.abs(m.value - v)
				if d < dist then
					best, dist = m.value, d
				end
			end
			return best
		end
		if step then
			v = lo + math.floor((v - lo) / step + 0.5) * step
		end
		return math.clamp(v, lo, hi)
	end
	local function set(v: number)
		v = snap(v)
		if v ~= value:peek() then
			value:set(v)
			if props.on_change then
				props.on_change(v)
			end
		end
	end
	local function at(local_x: number)
		local w = track.width or 1
		local usable = math.max(1, w - KNOB)
		set(lo + math.clamp((local_x - KNOB / 2) / usable, 0, 1) * (hi - lo))
	end
	local unit = step or (hi - lo) / 100

	local function knob_x(): number
		local w = track.width or 0
		return fraction() * math.max(0, w - KNOB)
	end

	local marks: { Node } = {}
	for _, m in props.marks or {} do
		local f = math.clamp((m.value - lo) / (hi - lo), 0, 1)
		table.insert(
			marks,
			ui.div {
				absolute = true,
				top = (KNOB - TRACK) / 2 + TRACK / 2 - 2,
				w = 4,
				h = 4,
				rounded = "$radius.full",
				x = function()
					return KNOB / 2 - 2 + f * math.max(0, (track.width or 0) - KNOB)
				end,
				bg = function()
					return if fraction() >= f then "$zeron.check.mark" else "$text.tertiary"
				end,
				opacity = 0.7,
				pointer_events = "none",
			}
		)
	end

	local groove = ui.div {
		ref = track,
		-- A slider to screen readers: its value in its range, stepped as a drag
		-- snaps, read as it's formatted, set through the same path a drag is.
		role = "slider",
		value = value,
		min = lo,
		max = hi,
		step = step,
		value_text = function(): string
			local v = value:get()
			return if props.format then props.format(v) else tostring(v)
		end,
		on_change = set,
		-- Its name and value, read together (a label follows what it reads).
		label = function()
			local v = value:get()
			local said = if props.format then props.format(v) else tostring(v)
			return if props.label then locale.s("ds.slider.name_value", { name = control.text(props.label) or "", value = said }) else said
		end,
		focusable = not props.disabled,
		relative = true,
		h = KNOB,
		w_full = true,
		cursor = if props.disabled then nil else "pointer",
		on_hover = function(inside: boolean)
			hovered:set(inside)
		end,
		on_press = function(e)
			if props.disabled then
				return
			end
			held:set(true)
			focused:set(false)
			at(e.local_x)
		end,
		on_pointer_move = function(e)
			if held:peek() then
				at(e.local_x)
			end
		end,
		on_release = function()
			held:set(false)
		end,
		on_key = function(e)
			focused:set(true)
			local big = if e.shift then 10 else 1
			if e.key == "ArrowRight" or e.key == "ArrowUp" then
				set(value:peek() + unit * big)
			elseif e.key == "ArrowLeft" or e.key == "ArrowDown" then
				set(value:peek() - unit * big)
			elseif e.key == "PageUp" then
				set(value:peek() + unit * 10)
			elseif e.key == "PageDown" then
				set(value:peek() - unit * 10)
			elseif e.key == "Home" then
				set(lo)
			elseif e.key == "End" then
				set(hi)
			else
				return false
			end
			return true
		end,
		-- The ring round the knob, not the groove (below).
		focus = {},
		-- The ring shows once a key reaches it (as zgx's `focus` does), not
		-- for a press.
		on_blur = function()
			focused:set(false)
		end,
		-- The groove, the knob's half-width in from each end, so the knob's
		-- centre travels it end to end.
		ui.div {
			absolute = true,
			left = KNOB / 2,
			right = KNOB / 2,
			top = (KNOB - TRACK) / 2,
			h = TRACK,
			rounded = "$radius.full",
			bg = "$zeron.switch.track",
			pointer_events = "none",
		},
		-- Its fill, up to the knob's centre.
		ui.div {
			absolute = true,
			left = KNOB / 2,
			top = (KNOB - TRACK) / 2,
			h = TRACK,
			rounded = "$radius.full",
			bg = "$zeron.switch.on",
			pointer_events = "none",
			w = function()
				return knob_x()
			end,
		},
		table.unpack(marks),
	}
	-- The knob, over the groove, wearing the focus ring.
	local ring = control.ring_when(function()
		return focused:get()
	end)
	local knob = ui.div {
		outline = ring.outline,
		outline_offset = ring.outline_offset,
		outline_color = ring.outline_color,
		absolute = true,
		left = 0,
		top = 0,
		w = KNOB,
		h = KNOB,
		rounded = "$radius.full",
		bg = "$zeron.switch.knob",
		shadow = "$zeron.switch.knob_lift",
		-- A hairline, so the white knob holds its edge on a pale window.
		border = 1,
		border_color = "$border.subtle",
		pointer_events = "none",
		x = knob_x,
		scale = ui.spring(ui.computed(function(): number
			return if held:get() then 1.15 elseif hovered:get() then 1.08 else 1
		end), { stiffness = 500, damping = 30 }),
	}

	local labels: Node? = nil
	if props.marks and #props.marks > 0 then
		local texts: { Node } = {}
		for _, m in props.marks do
			if m.label then
				local f = math.clamp((m.value - lo) / (hi - lo), 0, 1)
				-- Centred on its mark: a box of no width at the mark, the
				-- label centred in it, running over both its sides.
				table.insert(
					texts,
					ui.row {
						absolute = true,
						top = 0,
						left = 0,
						w = 0,
						justify = "center",
						x = function()
							return KNOB / 2 + f * math.max(0, (track.width or 0) - KNOB)
						end,
						ui.text {
							m.label,
							shrink = 0,
							text_size = "$font.size.xs",
							tabular = true,
							color = function()
								return if value:get() == m.value then "$text.secondary" else "$text.tertiary"
							end,
						},
					}
				)
			end
		end
		labels = ui.div { relative = true, h = 14, w_full = true, table.unpack(texts) }
	end

	local body = ui.column {
		relative = true,
		grow = 1,
		min_w = 0,
		gap = "$space.1",
		ui.div { relative = true, w_full = true, groove, knob },
		labels,
	}
	return ui.row {
		gap = "$space.3",
		items = "start",
		w = if props.w_full or props.grow then nil else (props.w or 240),
		w_full = props.w_full,
		grow = props.grow,
		disabled = props.disabled,
		opacity = if props.disabled then "$zeron.disabled" else nil,
		body,
		if props.format
			then ui.text {
				function()
					return (props.format :: (number) -> string)(value:get())
				end,
				shrink = 0,
				min_w = 36,
				align = "right",
				tabular = true,
				text_size = "$font.size.sm",
				line_height = KNOB,
				color = "$text.secondary",
			}
			else nil,
	}
end)
