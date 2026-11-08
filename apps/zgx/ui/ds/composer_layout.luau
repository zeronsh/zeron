-- Pure target geometry. No theme reads, signals, animation or history
-- mutations: callers resolve theme tokens and explicitly carry the text
-- expansion decision from one input change to the next.
local M = {}

local LINE, ONE_LINE = 22, 47
local TEXT_TOP, TEXT_BOTTOM, TEXT_MIN, TEXT_MAX = 16, 4, 76, 260
local CONTROL, TEXT_SIDE, TEXT_OPEN_SIDE = 28, 8, 16
local INSET_LINE = (ONE_LINE - CONTROL) / 2
local ACTIONS, ACTIONS_OVER, HYSTERESIS = 42, 3, 32
M.BORDER = 1

export type Sample = { unwrapped_width: number, wrapped_height: number, line_height: number }
export type Input = {
	width: number,
	sample: Sample?,
	trailing_width: number,
	-- Already resolved by control.nest(radius token, border), preserving
	-- control.nest's special handling of fully rounded corners.
	control_corner: number,
	header_height: number,
	header_gap: number,
	expanded: boolean,
	cargo: boolean,
	asking: boolean,
	dictating: boolean,
	multiline: boolean,
}
export type Geometry = {
	width: number,
	mode: "compact" | "expanded",
	expanded: boolean,
	next_text_expanded: boolean,
	natural_height: number,
	height: number,
	header_height: number,
	tray_inset: number,
	-- Constraints for deriving a safe viewport from a presented frame.
	line_height: number,
	compact_inset: number,
	compact_text: { left: number, right: number },
	text: { left: number, right: number, width: number, top: number, height: number },
	controls: {
		leading: { left: number, bottom: number, height: number },
		trailing: { right: number, bottom: number, height: number },
		voice: { left: number, right: number, bottom: number },
	},
}

function M.measurement(width: number)
	return { left = TEXT_OPEN_SIDE, width = math.max(0, width - M.BORDER * 2 - TEXT_OPEN_SIDE * 2) }
end

-- Also used by the footer/tray to line up with the pill's controls; their
-- own layout stays outside the text geometry calculation.
local function chrome_of(control_corner: number)
	local inset = control_corner - CONTROL / 2
	return {
		inset = inset,
		tray_inset = inset + M.BORDER,
		actions_height = math.max(ACTIONS, inset + CONTROL + ACTIONS_OVER),
	}
end

function M.calculate(p: Input, previous_text_expanded: boolean): Geometry
	local chrome = chrome_of(p.control_corner)
	local tail = if p.trailing_width > 0 then p.trailing_width else CONTROL * 2 + TEXT_SIDE
	local line_left = INSET_LINE + CONTROL + TEXT_SIDE
	local line_right = TEXT_SIDE + tail + INSET_LINE
	local room = p.width - M.BORDER * 2 - line_left - line_right
	local forced = p.expanded or p.cargo or p.asking or p.dictating
	local next_text_expanded = previous_text_expanded
	local expanded = forced
	if not forced and p.width > 0 then
		if p.multiline then
			expanded = true
		elseif p.sample == nil then
			expanded = previous_text_expanded
		elseif previous_text_expanded then
			expanded = p.sample.unwrapped_width > room - HYSTERESIS
		else
			expanded = p.sample.unwrapped_width > room
		end
		next_text_expanded = expanded
	end
	-- Forced modes and an unknown width don't change text hysteresis.
	-- This preserves the pre-refactor behavior after a question/dictation.
	local head = if p.cargo or p.asking then math.max(0, p.header_height) else 0
	local natural_text = if p.sample then math.max(p.sample.wrapped_height, p.sample.line_height) else LINE
	local natural_box = natural_text + TEXT_TOP + TEXT_BOTTOM
	local box = math.clamp(natural_box, TEXT_MIN, TEXT_MAX)
	local head_adjustment = if head > 0 then head + p.header_gap - TEXT_TOP else 0
	local height = if expanded then box + head_adjustment + chrome.actions_height + M.BORDER * 2 else ONE_LINE + M.BORDER * 2
	local natural_height = if expanded then natural_box + head_adjustment + chrome.actions_height + M.BORDER * 2 else height
	local left = if expanded then TEXT_OPEN_SIDE else line_left
	local right = if expanded then TEXT_OPEN_SIDE else line_right
	local inset = if expanded then chrome.inset else INSET_LINE
	return {
		width = math.max(0, p.width),
		mode = if expanded then "expanded" else "compact",
		expanded = expanded,
		next_text_expanded = next_text_expanded,
		-- Natural is before the text box's min/max; height is the limited
		-- target pill height. Neither is its animated/presented height.
		natural_height = natural_height,
		height = height,
		header_height = head,
		tray_inset = chrome.tray_inset,
		line_height = if p.sample then p.sample.line_height else LINE,
		compact_inset = INSET_LINE,
		compact_text = { left = line_left, right = line_right },
		text = {
			left = left,
			right = right,
			width = math.max(0, p.width - M.BORDER * 2 - left - right),
			top = if expanded then (if head > 0 then head + p.header_gap else TEXT_TOP) else (ONE_LINE - LINE) / 2,
			height = if expanded then box - TEXT_TOP - TEXT_BOTTOM else LINE,
		},
		controls = {
			leading = { left = inset, bottom = inset, height = CONTROL },
			trailing = { right = inset, bottom = inset - 2, height = 32 },
			voice = {
				left = inset + CONTROL + TEXT_SIDE,
				right = inset + CONTROL * 2 + TEXT_SIDE * 2,
				bottom = inset,
			},
		},
	}
end

return M
