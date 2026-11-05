-- A token field: a list of short things typed one at a time (emails to
-- invite, labels), each turning into a chip as it is finished: Enter, a
-- comma, a space or leaving the field. Backspace in an empty field takes the
-- last one back to edit. A pasted list splits into chips. `valid` says
-- whether a chip is good; one that isn't reads in the danger's ink, and stays
-- so it can be fixed. The field's frame, label, hint and error are a text
-- field's.
--
-- `TokenField { value = emails, label = "Invite by email", placeholder = "name@company.com",
--   valid = function(s) return string.find(s, "@", 1, true) ~= nil end }`

local Tag = require("@self/badge").Tag
local control = require("@self/control")
local field = require("@self/field")

export type Props = {
	value: State<{ string }>,
	-- (Each takes a message, `ui.t`.)
	label: Label?,
	placeholder: (string | Message)?,
	hint: Label?,
	error: Props_error,
	valid: ((s: string) -> boolean)?,
	size: control.Size?,
	-- Its accessible name when it has no label.
	name: (string | Message)?,
	disabled: boolean?,
	w: number?,
	w_full: boolean?,
	autofocus: boolean?,
}

-- Where a list breaks into chips.
local BREAK = "[,;%s]+"

return ui.component(function(props: Props): Node
	local size: control.Size = props.size or "md"
	local value = props.value
	local draft = ui.use_state("")
	local focused = ui.use_state(false)
	local hovered = ui.use_state(false)
	local input_ref = ui.ref()

	local function take()
		local list = table.clone(value:peek())
		local changed = false
		for part in string.gmatch(draft:peek() .. ",", "(.-)" .. BREAK) do
			if part ~= "" and not table.find(list, part) then
				table.insert(list, part)
				changed = true
			end
		end
		draft:set("")
		if changed then
			value:set(list)
		end
	end
	local function drop(token: string)
		local list = table.clone(value:peek())
		local at = table.find(list, token)
		if at then
			table.remove(list, at)
			value:set(list)
		end
	end

	-- The chips and, last among them, the input, so it wraps with them.
	local items = ui.computed(function(): { string }
		local out = table.clone(value:get())
		table.insert(out, "\0input")
		return out
	end)
	local input = ui.input(field.input({
		ref = input_ref,
		value = draft,
		label = props.label or props.name or props.placeholder or "",
		placeholder = function()
			return if #value:get() == 0 then control.text(props.placeholder) or "" else ""
		end,
		on_mount = if props.autofocus
			then function(el)
				el:focus()
			end
			else nil,
		on_commit = take,
		on_blur = take,
		on_key = function(e)
			if e.key == "Enter" or e.key == "," or e.key == " " or e.key == ";" then
				take()
				return true
			elseif e.key == "Backspace" and draft:peek() == "" then
				local list = value:peek()
				if #list > 0 then
					local last = list[#list]
					drop(last)
					draft:set(last)
					return true
				end
			end
			return false
		end,
	}, size, focused))

	-- One row of chips is a single-line field: the chips' height (a Tag's),
	-- the frame's (a control's) and the inset between them are one sum, the
	-- inset being what is left of the frame's height past its edge and a
	-- chip, halved. A chip's gap from the next is that inset too, so the
	-- chips sit in the frame as evenly as they sit beside each other. The
	-- frame grows only once the chips really wrap.
	local CHIP = if size == "sm" then 20 else 24
	local INSET = ui.raw((control.H[size] - 2 - CHIP) / 2, "the frame's height less its edge and a chip, halved")
	-- The chips and, last among them, the input, which asks for as much
	-- room as what is being typed (a hidden copy of the draft, measured) and
	-- a caret's worth more, a few characters' worth at least, and grows into
	-- whatever its line has left. So the caret stays on the chips' line
	-- until the typing truly runs out of room there, then takes a line of
	-- its own; a bare input would ask for its 240 whatever it held and wrap
	-- at once.
	local AT_LEAST, CARET = 32, 12
	local ghost = ui.ref()
	local box = field.frame {
		size = size,
		focused = focused,
		hovered = hovered,
		error = props.error,
		grows = true,
		w_full = true,
		px = INSET,
		py = INSET,
		gap = INSET,
		flex_wrap = true,
		items = "center",
		on_click = function()
			local el = input_ref:peek()
			if el then
				el:focus()
			end
		end,
		ui.each(items, function(token: string): Node
			if token == "\0input" then
				return ui.div {
					w = AT_LEAST,
					grow = 1,
					min_w = function()
						return math.max(AT_LEAST, (ghost.width or 0) + CARET)
					end,
					h = CHIP,
					justify = "center",
					input,
					ui.text {
						function()
							return draft:get()
						end,
						ref = ghost,
						absolute = true,
						opacity = 0,
						text_size = control.TEXT[size],
						aria_hidden = true,
						pointer_events = "none",
					},
				}
			end
			return Tag {
				token,
				invalid = props.valid ~= nil and not props.valid(token),
				on_remove = function()
					drop(token)
				end,
			}
		end, {
			key = function(token: string)
				return token
			end,
			direction = "row",
			flex_wrap = true,
			items = "center",
			gap = INSET,
			grow = 1,
			min_w = 0,
		}),
	}
	return field.wrap {
		label = props.label,
		hint = props.hint,
		error = props.error,
		size = size,
		box = box,
		disabled = props.disabled,
		w = if props.w_full then nil else (props.w or 320),
		w_full = props.w_full,
	}
end)
