-- A multi-line text field: the field's frame round a textarea that grows
-- with what is written, from `rows` lines to `max_rows`, and scrolls past
-- that. The height eases as it grows, measured from the wrapped text (as
-- the composer measures its draft).
--
-- `TextArea { value = state, label = "Description", rows = 3, max_rows = 8 }`.
-- `subject = { soft = 50, max = 72, body = 72 }` holds a commit message to
-- git's rules: the first line, the subject, wants 50 characters and has to
-- stop by 72; every line after it wraps by 72. What runs past a line's limit
-- takes the warning's ink (past a subject's hard limit, the danger's), on
-- whichever line it is, and the count says which line is over.

local locale = require("@self/locale")
local control = require("@self/control")
local field = require("@self/field")

export type Props = {
	value: State<string>,
	-- (Each takes a message, `ui.t`.)
	label: Label?,
	placeholder: (string | Message)?,
	hint: Label?,
	error: Props_error,
	count: field.Count?,
	-- A commit message's subject line: its length counted against these.
	subject: { soft: number, max: number?, body: number? }?,
	rows: number?,
	max_rows: number?,
	size: control.Size?,
	disabled: boolean?,
	name: (string | Message)?,
	autofocus: boolean?,
	-- ⌘Enter (a form's submit from a multi-line field).
	on_submit: ((text: string) -> ())?,
	on_key: ((e: any) -> boolean?)?,
	-- Markdown as it is typed, or a code editor's: a textarea `highlight`.
	highlight: any?,
	mono: boolean?,
	w: number?,
	w_full: boolean?,
	grow: number?,
}

-- Padding above and below the text inside the frame (zeron.space.tight,
-- in points for the height's sum).
local PAD_Y = 6

local function first_line(s: string): string
	return string.match(s, "^[^\n]*") or ""
end

return ui.component(function(props: Props): Node
	local size: control.Size = props.size or "md"
	local value = props.value
	local focused = ui.use_state(false)
	local hovered = ui.use_state(false)
	local measure = ui.ref()
	local input_ref = ui.ref()
	local line = ui.theme.get("zeron.field.line") :: number? or 20
	local rows, max_rows = props.rows or 3, props.max_rows or 10
	local font = if props.mono then "$font.mono" else nil

	local height = ui.computed(function(): number
		local h = measure.height or line
		return math.clamp(h, line * rows, line * max_rows) + PAD_Y * 2 + 2
	end)

	local count: field.Count? = props.count
	local subject = props.subject
	local body_limit = if subject then subject.body or 72 else 0
	-- The first body line past its limit: its number, and its length.
	local function long_line(): (number?, number)
		local n = 0
		for line in string.gmatch(value:get() .. "\n", "([^\n]*)\n") do
			n += 1
			local len = utf8.len(line) or #line
			if n > 1 and len > body_limit then
				return n, len
			end
		end
		return nil, 0
	end
	if subject and not count then
		local subject_count: field.Count = {
			of = function()
				return utf8.len(first_line(value:get())) or 0
			end,
			soft = subject.soft,
			max = subject.max,
			-- A subject over its limit speaks first; a good one gives way to
			-- a long line further down, named, or nothing would explain its ink.
			say = function(n: number): (string?, boolean?)
				local at, len = long_line()
				if at and n <= subject.soft then
					return locale.s("ds.text_area.line_over", { line = tostring(at), length = tostring(len), limit = tostring(body_limit) }), true
				end
				return nil, nil
			end,
		}
		count = subject_count
	end
	-- Past a line's limit: the characters beyond it in amber (past a
	-- subject's hard one, red). Mid steps of the palette, the same in both
	-- themes: a textarea keeps a line's colours until the line changes, so
	-- a theme's own ink would stay the theme it was typed in.
	local highlight = props.highlight
	if subject and not highlight then
		highlight = function(text: string, state: number)
			-- The first line is the subject; every other is held to the body's.
			local is_subject = state == 0
			local soft = if is_subject then subject.soft else body_limit
			local spans = {}
			local n = utf8.len(text) or #text
			if n > soft then
				local soft_at = utf8.offset(text, soft + 1) or (#text + 1)
				local hard = if is_subject then subject.max else nil
				local hard_at = if hard and n > hard then utf8.offset(text, hard + 1) else nil
				table.insert(spans, { soft_at, (hard_at or (#text + 1)) - 1, "$palette.amber.500" })
				if hard_at then
					table.insert(spans, { hard_at, #text, "$palette.red.500" })
				end
			end
			return spans, 1
		end
	end

	local area = ui.textarea(field.input({
		ref = input_ref,
		value = value,
		label = props.label or props.name or props.placeholder or "",
		placeholder = props.placeholder,
		error = props.error,
		wrap = true,
		highlight = highlight,
		font = font,
		line_height = line,
		on_mount = if props.autofocus
			then function(el)
				el:focus()
			end
			else nil,
		on_key = function(e)
			if props.on_key and props.on_key(e) then
				return true
			end
			if e.key == "Enter" and e.cmd and props.on_submit then
				props.on_submit(value:peek())
				return true
			end
			return false
		end,
	}, size, focused))

	local box = field.frame {
		size = size,
		focused = focused,
		hovered = hovered,
		error = props.error,
		h = ui.tween(height, { duration = 160, ease = "out(3)" }),
		py = "$zeron.space.tight",
		items = "stretch",
		relative = true,
		overflow = "hidden",
		w_full = true,
		on_click = function()
			local el = input_ref:peek()
			if el then
				el:focus()
			end
		end,
		area,
		-- The measure: the text wrapped as the field wraps it (a mark after
		-- it, so a last empty line counts), unseen.
		ui.text {
			ref = measure,
			absolute = true,
			left = control.PAD_PT[size],
			right = control.PAD_PT[size],
			top = 0,
			opacity = 0,
			pointer_events = "none",
			aria_hidden = true,
			wrap = true,
			font = font,
			text_size = control.TEXT[size],
			line_height = line,
			function()
				return value:get() .. "\u{200B}"
			end,
		},
	}
	return field.wrap {
		label = props.label,
		hint = props.hint,
		error = props.error,
		count = count,
		size = size,
		box = box,
		disabled = props.disabled,
		w = if props.w_full then nil else (props.w or 320),
		w_full = props.w_full,
		grow = props.grow,
	}
end)
