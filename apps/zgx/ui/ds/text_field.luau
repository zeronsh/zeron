-- A text field: one line of text in the field's frame (field.luau), with
-- the pieces a form needs. A label over it, a hint or an error under it, a
-- count of what is typed (warning past a soft limit, over past a hard one),
-- a leading icon, a clear button once there is text, a password's reveal,
-- and something of the caller's at its end (a key, a unit). Three sizes,
-- as Button's.
--
-- `TextField { value = state, label = "Name", placeholder = "Ada Lovelace" }`;
-- `password = true` masks it with a reveal; `clearable = true` puts a clear
-- button in it; `error` (a string, state or function) marks it wrong.

local locale = require("@self/locale")
local Icon = require("@self/icon")
local control = require("@self/control")
local field = require("@self/field")

export type Props = {
	value: State<string>,
	label: Label?,
	-- (A state or a function for words that follow something: zgx's own
	-- placeholder takes either.)
	placeholder: (Reactive<string> | Message)?,
	-- (Each of these may be a state or a function too, and is followed.)
	hint: (control.Live<string> | Message)?,
	error: Props_error,
	count: field.Count?,
	size: control.Size?,
	icon: Icon.Name?,
	clearable: boolean?,
	password: boolean?,
	-- Something of the caller's at its end (a Kbd, a unit, a spinner).
	trailing: Node?,
	disabled: Props_disabled,
	-- Its accessible name when it has no label to say it.
	name: Label?,
	-- A ref to the input itself, for what focuses or selects it from outside.
	input_ref: Ref?,
	inputmode: ("text" | "decimal" | "numeric" | "tel" | "search" | "email" | "url" | "none")?,
	select_on_focus: boolean?,
	autofocus: boolean?,
	on_commit: ((text: string) -> ())?,
	on_key: ((e: any) -> boolean?)?,
	-- Heard as the field takes the keyboard (true) and gives it up (false):
	-- a list it drives (a browser's suggestions) closes as it's left.
	on_focus_change: ((focused: boolean) -> ())?,
	-- A field that drives a list under it, for assistive technology: its
	-- role ("combobox"), whether the list shows, and the option lit in it
	-- (an id), followed as they change.
	role: string?,
	expanded: (() -> boolean)?,
	active_descendant: (() -> string?)?,
	-- While true, the field opens into what hangs under it (a browser's
	-- suggestions): its bottom corners square, its bottom edge gone, on the
	-- menus' solid surface (ds/field `joined`).
	joined: (() -> boolean)?,
	-- Escape clears it (a search's), as well as leaving it.
	escape_clears: boolean?,
	-- Its width (the frame's): `w`, `w_full`, `grow`, `max_w`.
	w: number?,
	w_full: boolean?,
	grow: number?,
	max_w: number?,
	-- The frame's own shape (a search's pill).
	rounded: any?,
	-- No frame of its own: a field that is a row of what holds it (a list's
	-- filter under its card's head), its words, icon and clear button as
	-- ever, but no focus ring: the card round it frames it, and the caret
	-- says it has focus. Its corner is still the frame's (its clear
	-- button's), so a holder nests it as it does a framed one.
	quiet: boolean?,
	-- Its own room before the icon, in points, where the field's glyph
	-- must land on an edge (a card's rows' words): its size's by default.
	pl: number?,
}

-- A small square button inside a field (clear, reveal): no fill until the
-- pointer is on it, as quiet as the placeholder until then.
local function side_room(size: control.Size): number
	return (control.H[size] - (if size == "sm" then 20 else 24)) / 2
end

local function inner(icon: () -> Icon.Name, label: Label, size: control.Size, corner: any, on_click: () -> ()): Node
	local box = if size == "sm" then 20 else 24
	return ui.button {
		label = label,
		w = box,
		h = box,
		px = 0,
		py = 0,
		shrink = 0,
		rounded = ui.raw(control.nest(corner, side_room(size)), "concentric with the field: its corner less the room round the button"),
		bg = "#00000000",
		color = "$text.tertiary",
		cursor = "pointer",
		hover = { bg = "$hover", color = "$text.secondary" },
		active = { bg = "$pressed", color = "$text.primary" },
		focus = control.ring,
		-- (Mouse presses don't take focus from the field.)
		on_click = on_click,
		justify = "center",
		Icon { name = icon, size = control.ICON[size] - 2 },
	}
end

return ui.component(function(props: Props): Node
	local size: control.Size = props.size or "md"
	local value = props.value
	local focused = ui.use_state(false)
	if props.on_focus_change then
		local heard = focused:peek()
		ui.effect(function()
			local now = focused:get()
			if now ~= heard then
				heard = now
				props.on_focus_change(now)
			end
		end)
	end
	local hovered = ui.use_state(false)
	local revealed = ui.use_state(false)
	local input_ref = props.input_ref or ui.ref()

	local function focus_input()
		local el = input_ref:peek()
		if el then
			el:focus()
		end
	end

	local function input(secure: boolean): Node
		return ui.input(field.input({
			ref = input_ref,
			value = value,
			-- (Named by its placeholder, as a plain or changing string, or a
			-- message.)
			label = props.label or props.name or (if type(props.placeholder) == "string" or type(props.placeholder) == "function" or control.is_message(props.placeholder) then props.placeholder else nil) or "",
			placeholder = props.placeholder,
			secure = secure,
			error = props.error,
			inputmode = props.inputmode,
			select_on_focus = props.select_on_focus,
			on_commit = props.on_commit,
			role = props.role,
			expanded = props.expanded,
			active_descendant = props.active_descendant,
			on_mount = if props.autofocus
				then function(el)
					el:focus()
				end
				else nil,
			on_key = function(e)
				if props.on_key and props.on_key(e) then
					return true
				end
				if e.key == "Escape" and props.escape_clears and value:peek() ~= "" then
					value:set("")
					return true
				end
				return false
			end,
		}, size, focused))
	end

	local children: { any } = {}
	if props.icon then
		-- Leading: the label's quiet ink, the accent's never (it isn't a
		-- control), firming as the field takes focus.
		table.insert(
			children,
			ui.div {
				shrink = 0,
				y = -0.5,
				-- (Decoration: the field is named by its label.)
				aria_hidden = true,
				color = function()
					return if focused:get() then "$text.secondary" else "$text.tertiary"
				end,
				Icon { name = props.icon, size = control.ICON[size] },
			}
		)
	end
	if props.password then
		-- Masked or shown: the same text, one input each way (zgx's mask is
		-- set as the input is made), focus kept as it turns.
		table.insert(
			children,
			ui.show(function()
				return revealed:get()
			end, input(false), input(true))
		)
	else
		table.insert(children, input(false))
	end
	if props.clearable then
		table.insert(
			children,
			ui.show(function()
				return value:get() ~= "" and not control.value(props.disabled)
			end, inner(function()
				return "close"
			end, locale.t("common.clear"), size, props.rounded or control.RADIUS[size], function()
				value:set("")
				focus_input()
			end))
		)
	end
	if props.password then
		table.insert(
			children,
			inner(function()
				return if revealed:get() then "eye-closed" else "eye"
			end, locale.t("ds.text_field.show_password"), size, props.rounded or control.RADIUS[size], function()
				local was = focused:peek()
				revealed:toggle()
				if was then
					focus_input()
				end
			end)
		)
	end
	if props.trailing then
		table.insert(children, ui.div { shrink = 0, color = "$text.tertiary", props.trailing })
	end

	local frame: { [any]: any } = {
		size = size,
		focused = focused,
		hovered = hovered,
		error = props.error,
		rounded = props.rounded,
		joined = props.joined,
		-- Quiet: no edge, no well (the hover's included).
		border = if props.quiet then 0 else nil,
		bg = if props.quiet then "#00000000" else nil,
		-- (Quiet, no focus ring either: it heads a card that frames it, and
		-- a click into it shows the caret, which is enough.)
		outline = if props.quiet then 0 else nil,
		outline_color = if props.quiet then "#00000000" else nil,
		pl = if props.pl then ui.raw(props.pl, "the room its holder asked for before the icon") else nil,
		-- Beside a button inside it, the room over and under that button
		-- (less the frame's edge), so the button sits evenly in the corner.
		pr = if props.clearable or props.password or props.trailing then ui.raw(side_room(size) - (if props.quiet then 0 else 1), "even with the button's top and foot") else nil,
		on_click = focus_input,
		w_full = true,
		table.unpack(children),
	}
	local box = field.frame(frame)
	return field.wrap {
		label = props.label,
		hint = props.hint,
		error = props.error,
		count = props.count,
		size = size,
		box = box,
		disabled = props.disabled,
		w = if props.w_full or props.grow then nil else (props.w or 280),
		w_full = props.w_full,
		grow = props.grow,
		-- Growing, it gives way too: an input alone is 240 wide, and a row
		-- wouldn't take that back.
		min_w = if props.grow then 0 else nil,
		max_w = props.max_w,
	}
end)
