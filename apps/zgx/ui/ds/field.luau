-- A field's frame, which every field shares (a text field, a multi-line
-- one, search, a number stepper, a select's trigger): a label over it, a
-- hint or an error under it with a count at the end, and the box between,
-- a quiet well whose hairline firms under the pointer and takes the accent
-- with the focus ring while focus is inside. An error turns the edge and
-- the ring to the danger's, and says why where the hint was.
--
-- `field.frame { size, focused, error, … children }` makes the box;
-- `field.wrap { label, hint, error, count, box }` lays the label and the
-- notes round it. Components build on these; screens use the fields.

local Icon = require("@self/icon")
local Hint = require("@self/hint")
local control = require("@self/control")
local material = require("@self/material")
local EditorMenu = require("@self/editor_menu")

local M = {}

export type Count = {
	-- What is counted (a function of the field's text, its first line's
	-- length for a commit's subject).
	of: () -> number,
	-- Past `soft` it warns, past `max` it is over.
	soft: number?,
	max: number?,
	-- Says something else in its place, given the count: the words, and
	-- whether they warn.
	say: ((n: number) -> (string?, boolean?))?,
}

-- The frame: a row (its own props refine the defaults) told the field's
-- size, its input's focus and the pointer over it (states the field sets),
-- and whether it is wrong (`error`, a row's: a value, state or function).
-- `grows`: as tall as it needs, a control's height at least.
export type FrameProps = Merge<ContainerProps, {
	size: control.Size?,
	focused: State<boolean>,
	hovered: State<boolean>,
	grows: boolean?,
	-- While true, joined to what opens under it (see `frame`).
	joined: (() -> boolean)?,
}>

-- The label, the box and the notes round it, and the width they share.
export type WrapProps = {
	label: any,
	hint: any,
	error: Props_error,
	count: Count?,
	size: control.Size?,
	box: Node,
	-- A value, state or function: the field dims and takes no input while
	-- it holds (a role that can't edit, a save on its way).
	disabled: Props_disabled,
	w: Props_w,
	w_full: boolean?,
	grow: number?,
	min_w: Props_min_w,
	max_w: Props_max_w,
	shrink: number?,
}

-- What a field's notes say: under it, the error (in the danger's ink, a
-- glyph before it) or the hint; the count at the end.
local function notes(props: WrapProps): Node?
	local hint, err, count = props.hint, props.error, props.count
	if not hint and not err and not count then
		return nil
	end
	local function problem(): string?
		return control.read(err)
	end
	local count_node = nil
	if count then
		local function tone(): string
			local n = count.of()
			if count.say then
				local _, warns = count.say(n)
				if warns then
					return "$zeron.tone.warning.ink"
				end
			end
			if count.max and n > count.max then
				return "$zeron.tone.danger.ink"
			elseif count.soft and n > count.soft then
				return "$zeron.tone.warning.ink"
			end
			return "$text.tertiary"
		end
		count_node = ui.text {
			function()
				local n = count.of()
				local said = if count.say then count.say(n) else nil
				if said then
					return said
				end
				local limit = count.soft or count.max
				return if limit then `{n}/{limit}` else tostring(n)
			end,
			shrink = 0,
			tabular = true,
			text_size = "$font.size.xs",
			line_height = "$font.line.xs",
			color = tone,
		}
	end
	return ui.row {
		gap = "$space.2",
		items = "start",
		ui.column {
			grow = 1,
			min_w = 0,
			if err
				then ui.show(problem, ui.row {
					gap = "$space.1",
					items = "center",
					color = "$zeron.tone.danger.ink",
					live = "polite",
					-- Half a point up: an icon beside text sits on its
					-- x-height, not its box.
					ui.div { y = -0.5, Icon { name = "danger-triangle", size = 12 } },
					ui.text {
						function()
							return problem() or ""
						end,
						text_size = "$font.size.sm",
						line_height = "$font.line.sm",
						wrap = true,
					},
				}, if hint then M.hint(hint) else nil)
				elseif hint then M.hint(hint)
				else nil,
		},
		count_node,
	}
end

-- A hint under a field: the design system's Hint.
function M.hint(text: any): Node
	return Hint { text }
end

-- The label over a field.
function M.label(text: any, size: control.Size?): Node
	return ui.text {
		text,
		text_size = if size == "lg" then "$font.size.md" else "$font.size.sm",
		line_height = if size == "lg" then "$font.line.md" else "$font.line.sm",
		weight = "$font.weight.medium",
		color = "$text.secondary",
	}
end

-- The label, the box and its notes, stacked: `box` is the frame.
function M.wrap(props: WrapProps): Node
	local label = props.label
	local n = notes(props)
	return ui.column {
		gap = "$zeron.space.tight",
		w = props.w,
		w_full = props.w_full,
		grow = props.grow,
		min_w = props.min_w,
		max_w = props.max_w,
		shrink = props.shrink,
		disabled = props.disabled,
		opacity = if props.disabled
			then function(): any
				return if control.value(props.disabled) then "$zeron.disabled" else nil
			end
			else nil,
		if label then M.label(label, props.size) else nil,
		props.box,
		n,
	}
end

-- The box: `focused` and `hovered` are states the caller's input sets (its
-- focus and the pointer over the frame), `error` a value, state or function
-- that is set while it is wrong. Its own props (`h`, `px`, `rounded`…)
-- refine the defaults for its size.
function M.frame(props: FrameProps): Node
	local size: control.Size = props.size or "md"
	local focused = props.focused
	local hovered = props.hovered
	local err = props.error
	local function invalid(): boolean
		local e = control.read(err)
		return if e and e ~= "" then true else false
	end
	-- Joined to what opens under it (`joined`, a function): its bottom
	-- corners square and its bottom edge gone, on the menus' solid surface,
	-- so the field and what hangs from it read as one shape.
	local joined: () -> boolean = props.joined or function(): boolean
		return false
	end
	local p: { [any]: any } = {
		h = control.H[size],
		px = control.PAD[size],
		gap = "$space.2",
		items = "center",
		rounded = control.RADIUS[size],
		border = 1,
		bg = function()
			if joined() then
				return "$zeron.menu.solid"
			end
			return if material.see_through() then (if hovered:get() and not focused:get() then "$zeron.field.glass_hover" else "$zeron.field.glass") elseif hovered:get() and not focused:get() then "$zeron.field.hover" else "$zeron.field.bg"
		end,
		border_color = function()
			if invalid() then
				return "$zeron.field.edge_error"
			elseif focused:get() or joined() then
				return "$zeron.field.edge_focus"
			elseif hovered:get() then
				return "$zeron.field.edge_hover"
			end
			return "$zeron.field.edge"
		end,
		cursor = "text",
		-- A press anywhere on it focuses its input: a group round the input
		-- (which carries the name and takes the focus), not a control of
		-- its own, and no Tab stop.
		role = "group",
		focusable = false,
		on_hover = function(inside: boolean)
			hovered:set(inside)
		end,
	}
	-- (Joined, no ring: it would ring the field alone, not the shape.)
	local function has_focus(): boolean
		return focused:get() and not joined()
	end
	for k, v in control.ring_when(has_focus, invalid) do
		p[k] = v
	end
	for k, v in props :: { [string | number]: unknown } do
		if k ~= "size" and k ~= "focused" and k ~= "hovered" and k ~= "error" and k ~= "grows" and k ~= "joined" then
			p[k] = v
		end
	end
	if props.joined then
		-- Its corner as a number (a pill's is half its height), for the two
		-- that square while joined.
		local r = p.rounded
		local corner: number = if type(r) == "number"
			then r
			elseif r == "$radius.full" then (control.H[size] :: number) / 2
			else ((ui.theme.get(string.sub(tostring(r), 2)) :: number?) or 0)
		-- (All four from one value, `corners`: a reactive single corner
		-- doesn't merge with a static `rounded`, and the field went an odd
		-- shape at rest.)
		p.rounded = nil
		p.corners = function(): { [string]: number }
			local low = if joined() then 0 else corner
			return { tl = corner, tr = corner, br = low, bl = low }
		end
		p.border_sides = function(): { number }
			return if joined() then { 1, 1, 0, 1 } else { 1, 1, 1, 1 }
		end
	end
	-- As tall as it needs, a control's height at least (chips wrapping).
	if props.grows then
		p.min_h = p.h
		p.h = nil
	end
	return ui.row(p)
end

-- The props an input inside a frame takes: no box of its own, the frame's
-- type, the accent caret, and the frame told of its focus.
function M.input(props: InputProps, size: control.Size, focused: State<boolean>): InputProps
	local p = table.clone(props)
	p.bg = "#00000000"
	p.border = 0
	p.rounded = 0
	p.px = 0
	p.py = 0
	p.grow = 1
	p.min_w = 0
	p.h_full = true
	p.focus = { border = 0, outline = 0 }
	p.text_size = control.TEXT[size]
	p.color = "$text.primary"
	p.placeholder_color = "$zeron.field.placeholder"
	p.caret_color = "$accent"
	local on_focus, on_blur = props.on_focus, props.on_blur
	p.on_focus = function(...)
		focused:set(true)
		-- (An `Action` is Rust's to run: only a function is called here.)
		if type(on_focus) == "function" then
			on_focus(...)
		end
	end
	p.on_blur = function(...)
		focused:set(false)
		if type(on_blur) == "function" then
			on_blur(...)
		end
	end
	-- The design system's text menu (ds/editor_menu), not zgx's own.
	return EditorMenu.on(p)
end

return M
