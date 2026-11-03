-- A code editor: a file's text, coloured as it is typed, on zgx's text
-- engine, with what the engine doesn't do built round it. The lines are the
-- engine's to shape, draw and edit, so a keystroke, a caret, a selection and
-- an input method are what they are in any field; `ds/code_view` decides
-- which lines it is given: a window of a few screens of the file, with the
-- folded lines left out and room left where a block stands between two
-- lines. This scroll view moves that window, so the file can be any length
-- and the gutter beside it (numbers, the fold controls) is drawn here.
--
-- What it adds is a code editor's hands. Enter keeps the line's indent (one
-- more after an opener, the closer put on a line of its own); Backspace in
-- an indent takes a whole step; Tab and ⇧Tab indent and outdent the lines a
-- selection touches; ⌘/ comments them; ⌥↑ ⌥↓ move them, ⇧⌥↑ ⇧⌥↓ copy them;
-- brackets close themselves and are stepped over, and the pair round the
-- caret is marked. ⌘F finds (case, whole words, a pattern; every match
-- marked, the current one firmer), ⌥⌘F replaces, ⌃G goes to a line. ⌥Z wraps
-- long lines; the gutter's numbers follow. ⌘S saves.
--
-- Code folds where its shape says (`ds/code_folds`: functions, blocks,
-- indentation, runs of comments and imports). A chevron shows in the gutter
-- beside a line that opens a region, under the pointer, and stays while it
-- is folded; the folded line reads `fn add(a, b) { ⋯ }  12 lines`, and a
-- click on that summary opens it. ⌥⌘[ and ⌥⌘] fold and open the region round
-- the caret, ⇧⌥⌘[ and ⇧⌥⌘] all of them. Folded lines are only out of sight:
-- find reaches them (and opens them), and a selection over one copies and
-- deletes what it hides.
--
-- A block stands between two lines and pushes the rest down: a comment being
-- written, a thread, an agent's note (`ds/inline_block` makes them; anything
-- fits). `blocks` lists them, each `{ key, line, … }`, under `line`, and
-- `block_view(block, { inset, joined })` draws one.
--
-- Read-only, it is a viewer: no caret, nothing typed lands, but a selection,
-- copying, folding, finding and going to a line all work. Its status bar
-- (where the caret is, the indent, the language, whether it changed) is off
-- there by default; the viewer's own header says what it shows.
--
-- `CodeEditor { value = state, language = "rust", readonly = false,
-- line_numbers = true, wrap = false, reveal = state, find_open = state,
-- status = true, on_save = function(text) … end, blocks = state,
-- block_view = function(block, context) … end, on_add_block = function(line)
-- … end, on_block_moved = function(key, line) … end, commands = table }`.
-- `language` is `Syntax.language(path)`'s (a string, a state or a function,
-- followed); `reveal`, a line to go to from outside (scrolled to the middle,
-- washed as it lands, then set back to nil); `dirty`, a state told whether
-- the text differs from what was saved; `saved`, a state holding the text
-- last saved, if something else keeps it; `commands`, a table it fills with
-- what a header's buttons ask of it (`save`, `fold_all`, `unfold_all`,
-- `go_to_line`); `place`, `{ get, set }` keeping where it stands (scroll and
-- selection) outside it. Box props (`grow`, `h`, `w_full`…) go to its frame. It fills
-- what it is given and scrolls inside, both ways.

local Icon = require("@self/icon")
local Kbd = require("@self/kbd")
local FadeText = require("@self/fade_text")
local IconButtonModule = require("@self/icon_button")
local IconButton = IconButtonModule.IconButton
local ToggleButton = require("@self/toggle_button")
local Button = require("@self/button")
local TextField = require("@self/text_field")
local ContextMenu = require("@self/context_menu")

local Folds = require("@self/code_folds")
local View = require("@self/code_view")
local Syntax = require("@self/syntax")
local Inline = require("@self/inline_block")
local locale = require("@self/locale")
local EditorMenu = require("@self/editor_menu")
local look = require("@self/look")

-- One thing standing between two lines: under `line`, drawn by `view`.
export type Block = { key: string, line: number, [any]: any }
-- `reveal`: the editor's scroll offset and its viewport's box, for a block
-- that must be seen as it opens (ds/inline_block's Composer brings itself
-- into view through them).
export type BlockContext = { inset: () -> number, joined: boolean, reveal: { offset: State<number>, frame: () -> any }? }
export type BlockView = (block: Block, context: BlockContext) -> Node

type Row = View.Row

export type Props = {
	value: State<string>,
	language: (string | State<string?> | (() -> string?))?,
	readonly: boolean?,
	line_numbers: boolean?,
	wrap: (boolean | State<boolean>)?,
	on_save: ((text: string) -> ())?,
	-- A line to go to (the caret there, scrolled to, its row washed): a
	-- number, or `{ line, column?, end_line? }` (an end line selects the
	-- lines from `line` through it). Set back to nil once it's gone to.
	reveal: State<any>?,
	find_open: State<boolean>?,
	status: boolean?,
	-- Folding off: no regions, no controls.
	folding: boolean?,
	blocks: (State<{ Block }> | (() -> { Block }))?,
	block_view: BlockView?,
	-- A + beside the line under the pointer asks for a block there.
	on_add_block: ((line: number) -> ())?,
	-- Edits carried a block to another line.
	on_block_moved: ((key: string, line: number) -> ())?,
	dirty: State<boolean>?,
	-- The text saved, if it is kept outside (a file's, kept as files change
	-- under an editor): otherwise it is what the editor was last given.
	saved: State<string>?,
	commands: { [string]: () -> () }?,
	-- What assistive technology calls it ("Code", or the file's name).
	label: string?,
	-- The code's size in points (Settings' code font size), its rows
	-- scaled with it; nil, the design system's (`$zeron.editor.size`). Read
	-- once: a new size is a new editor.
	text_size: number?,
	-- How the keyboard leaves it (Tab indents), read after its name: e.g.
	-- "Escape moves to the file's tab".
	escape_hint: string?,
	-- Where it stands (its scroll and selection, in file bytes), if the
	-- caller keeps it: an editor made again (the screen reloaded, the
	-- file opened again) comes back there.
	place: Place?,
	[any]: any,
}

export type Spot = { offset: number, anchor: number, focus: number }
export type Place = { get: () -> Spot?, set: (Spot) -> () }

-- Matches found by a pattern, or counted, at most this many (the engine's
-- own search stops there too): past it the count says "10000+".
local LIMIT = 10000
-- How far either way a bracket looks for its pair before giving up (a
-- bracket left open at the top of a long file shouldn't scan all of it).
local BRACKET_REACH = 64 * 1024
-- How long a line gone to stays washed, and how long its wash fades.
local REVEAL_HOLD, REVEAL_FADE = 700, 500
-- How long a pattern's search waits for typing to stop before it runs
-- again over the whole text.
local SETTLE = 90
-- The find bar: the pane's width under which it folds, the widest its fields
-- get, and the room its count takes in the field at most.
local FIND_FOLD, FIND_FIELD, FIND_TALLY = 440, 360, 96
-- How long after typing stops the regions are found again, and how the scan
-- shares the frames: this long at a time, this many milliseconds apart.
local RESCAN, SCAN_SECONDS, SCAN_REST = 240, 0.003, 16
-- The rows held past each end of the view, and how near the view comes to
-- the window's end before it moves. A wider window travels further between
-- moves and costs more each time the textarea draws.
local OVERSCAN, MARGIN = 40, 12
-- Empty rows after the last line, so it never has to sit on the pane's edge.
local TAIL = 4
-- A dragged selection past the view's edge: how often it moves, how far at most.
local DRAG_TICK, DRAG_MAX = 16, 48
-- The gutter: the room before the numbers, the fold control's column and the
-- gap between them. With `on_add_block`, the + has a slot of its own before
-- the numbers, there whether it shows or not, so nothing moves when it does.
local GUTTER_INSET, FOLD_SLOT, NUMBER_GAP = 8, 18, 2
-- A step of undo: how long typing runs together, how many steps are kept.
local RUN_SECONDS, HISTORY = 1, 2000
-- Every this many lines, what the highlighter carries is remembered, so a
-- window opened deep in a file starts inside the comment it is in; found in
-- the background, this long at a time, this many milliseconds apart.
local CHAIN_STEP, CHAIN_SECONDS, CHAIN_REST = 128, 0.002, 16

-- The code's size and a row's height for `size` points: the design
-- system's (`$zeron.editor.size` on `$zeron.editor.line`) at nil, rows
-- scaled with it otherwise, to the whole point.
local function metrics(size: number?): (number, number)
	local base = (ui.theme.get("zeron.editor.size") :: number?) or 13
	local line = (ui.theme.get("zeron.editor.line") :: number?) or 20
	if not size or size <= 0 or size == base then
		return base, line
	end
	return size, math.floor(size * line / base + 0.5)
end

-- The editor's own props; the frame takes the rest.
local OWN = {
	value = true,
	text_size = true,
	escape_hint = true,
	language = true,
	readonly = true,
	line_numbers = true,
	wrap = true,
	on_save = true,
	reveal = true,
	find_open = true,
	status = true,
	folding = true,
	blocks = true,
	block_view = true,
	on_add_block = true,
	on_block_moved = true,
	dirty = true,
	saved = true,
	commands = true,
	label = true,
	place = true,
	children = true,
}

-- How the language comments a line (the highlighter knows; plain text
-- takes //, as most code does).
local function comment_of(language: string?): { line: string?, block: { string }? }
	return Syntax.comment(language) or { line = "//" }
end

---------------------------------------------------------------------------
-- The text, read where it is: offsets are bytes from 0 (as the editor's
-- selection gives them), so byte `p` of the string is just before offset p.

-- Where the line holding offset `p` starts. A long line is searched back a
-- few kilobytes at a time (in C), never a byte at a time.
-- How many columns `s` takes as the editor draws it: a character each, a
-- tab to the next multiple of 8 (a textarea's).
local function display_cols(s: string): number
	if not string.find(s, "\t", 1, true) then
		return utf8.len(s) or #s
	end
	local col = 0
	for _, c in utf8.codes(s) do
		if c == 9 then
			col = (col // 8 + 1) * 8
		else
			col += 1
		end
	end
	return col
end

-- The byte in `line` (from 0) where display column `col` (from 0) falls.
local function byte_at_col(line: string, col: number): number
	local at = 0
	for pos, c in utf8.codes(line) do
		local next_at = if c == 9 then (at // 8 + 1) * 8 else at + 1
		if next_at > col then
			-- (Nearer this character's end: after it.)
			return if col - at >= (next_at - at) / 2 then pos - 1 + #utf8.char(c) else pos - 1
		end
		at = next_at
	end
	return #line
end

-- The widest line of `t`, in columns.
local function widest(t: string): number
	local most = 0
	for line in string.gmatch(t, "[^\n]*") do
		local n = display_cols(line)
		if n > most then
			most = n
		end
	end
	return most
end

local function line_start(t: string, p: number): number
	while p > 0 do
		local from = math.max(1, p - 4095)
		local at = string.match(string.sub(t, from, p), ".*()\n")
		if at then
			return from + at - 1
		end
		p = from - 1
	end
	return 0
end

-- Where the line holding offset `p` ends (before its newline).
local function line_end(t: string, p: number): number
	local nl = string.find(t, "\n", p + 1, true)
	return if nl then nl - 1 else #t
end

-- The whole lines a selection touches, `s` to `e` (a selection ending at a
-- line's very start leaves that line out, as every editor does).
local function lines_of(t: string, lo: number, hi: number): (number, number)
	local last = hi
	if hi > lo and string.byte(t, hi) == 10 then
		last = hi - 1
	end
	return line_start(t, lo), line_end(t, last)
end

-- How the text indents: a tab, or two or four spaces, judged from its
-- first lines (a file's own habit beats any default).
local function detect_indent(t: string): string
	local sample = string.sub(t, 1, 64 * 1024)
	local tabs, spaces, odd = 0, 0, 0
	for lead in string.gmatch(sample, "\n([ \t]+)%S") do
		if string.byte(lead, 1) == 9 then
			tabs += 1
		else
			spaces += 1
			if #lead % 4 ~= 0 then
				odd += 1
			end
		end
	end
	if tabs > spaces then
		return "\t"
	elseif spaces > 0 and odd * 5 > spaces then
		return "  "
	end
	return "    "
end

-- A string as a pattern that finds it as written.
local function escape(s: string): string
	return (string.gsub(s, "[%^%$%(%)%%%.%[%]%*%+%-%?]", "%%%0"))
end

-- A pattern that ignores ASCII letters' case: each letter a class of both.
local function fold(pattern: string): string
	local out = table.create(#pattern)
	local i, n = 1, #pattern
	local in_class = false
	while i <= n do
		local c = string.sub(pattern, i, i)
		if c == "%" then
			table.insert(out, string.sub(pattern, i, i + 1))
			i += 2
			continue
		end
		if c == "[" then
			in_class = true
		elseif c == "]" then
			in_class = false
		end
		if not in_class and string.match(c, "%a") then
			table.insert(out, `[{string.lower(c)}{string.upper(c)}]`)
		else
			table.insert(out, c)
		end
		i += 1
	end
	return table.concat(out)
end

local WORD = "[%w_]"
local function is_word(c: string): boolean
	return string.match(c, "^[%w_]$") ~= nil
end

-- A regular expression as a Luau pattern, as far as one goes: classes
-- (\d \w \s, [a-z]), . * + ? and lazy *?, {n,m}, \b, ^ and $ at a line's
-- ends, and groups (captured, for $1 in a replacement) that don't repeat.
-- Alternation has no Luau equivalent: it says so instead of guessing.
local function translate(re: string): (string?, string?)
	local out: { string } = {}
	local last: string? = nil -- the last atom, for {n,m} and +?
	local i, n = 1, #re
	local function atom(a: string)
		table.insert(out, a)
		last = a
	end
	local function class_escape(c: string): string
		if c == "d" then
			return "%d"
		elseif c == "D" then
			return "%D"
		elseif c == "w" then
			return "%w_"
		elseif c == "s" then
			return "%s"
		elseif c == "n" then
			return "\n"
		elseif c == "t" then
			return "\t"
		end
		return if string.match(c, "%w") then c else "%" .. c
	end
	while i <= n do
		local c = string.sub(re, i, i)
		if c == "\\" then
			local d = string.sub(re, i + 1, i + 1)
			i += 2
			if d == "" then
				return nil, locale.s("ds.code_editor.pattern.ends_in_backslash")
			elseif d == "b" then
				local prev = string.sub(re, i - 3, i - 3)
				local starts = #out == 0 or not (is_word(prev) or prev == "w" or prev == ")" or prev == "]")
				table.insert(out, if starts then "%f[%w_]" else "%f[^%w_]")
			elseif d == "w" then
				atom(WORD)
			elseif d == "W" then
				atom("[^%w_]")
			elseif d == "d" or d == "D" or d == "s" or d == "S" then
				atom("%" .. d)
			elseif d == "n" then
				atom("\n")
			elseif d == "t" then
				atom("\t")
			else
				atom(if string.match(d, "%w") then d else "%" .. d)
			end
		elseif c == "[" then
			local j = i + 1
			local parts = { "[" }
			if string.sub(re, j, j) == "^" then
				table.insert(parts, "^")
				j += 1
			end
			local closed = false
			while j <= n do
				local d = string.sub(re, j, j)
				if d == "]" and #parts > (if parts[2] == "^" then 2 else 1) then
					closed = true
					break
				elseif d == "\\" then
					table.insert(parts, class_escape(string.sub(re, j + 1, j + 1)))
					j += 2
				elseif d == "%" or d == "]" then
					table.insert(parts, "%" .. d)
					j += 1
				else
					table.insert(parts, d)
					j += 1
				end
			end
			if not closed then
				return nil, locale.s("ds.code_editor.pattern.bracket_open")
			end
			table.insert(parts, "]")
			atom(table.concat(parts))
			i = j + 1
		elseif c == "." then
			atom("[^\n]")
			i += 1
		elseif c == "*" or c == "+" or c == "?" then
			if not last then
				return nil, locale.s("ds.code_editor.pattern.nothing_to_repeat", { c = c })
			end
			local lazy = string.sub(re, i + 1, i + 1) == "?"
			if lazy and c == "*" then
				table.insert(out, "-")
			elseif lazy and c == "+" then
				table.insert(out, (last :: string) .. "-")
			else
				table.insert(out, c)
			end
			last = nil
			i += if lazy then 2 else 1
		elseif c == "{" then
			local lo_s, hi_s, close = string.match(re, "^{(%d*),?(%d*)()}", i)
			local comma = string.find(string.sub(re, i, (close or i)), ",", 1, true) ~= nil
			if not lo_s or not last then
				atom("{")
				i += 1
				continue
			end
			local base = last :: string
			table.remove(out)
			local lo_n = tonumber(lo_s) or 0
			for _ = 1, lo_n do
				table.insert(out, base)
			end
			if comma and hi_s == "" then
				table.insert(out, base .. "*")
			elseif comma then
				for _ = lo_n + 1, tonumber(hi_s) or lo_n do
					table.insert(out, base .. "?")
				end
			end
			last = nil
			i = (close :: number) + 1
		elseif c == "(" then
			if string.sub(re, i + 1, i + 2) == "?:" then
				i += 3
			else
				table.insert(out, "(")
				i += 1
			end
			last = nil
		elseif c == ")" then
			local q = string.sub(re, i + 1, i + 1)
			if q == "*" or q == "+" or q == "?" or q == "{" then
				return nil, locale.s("ds.code_editor.pattern.group_repeat")
			end
			-- A non-capturing group's close leaves nothing.
			local opens, closes = 0, 0
			for _ in string.gmatch(table.concat(out), "%(") do
				opens += 1
			end
			for _ in string.gmatch(table.concat(out), "%)") do
				closes += 1
			end
			if opens > closes then
				table.insert(out, ")")
			end
			last = nil
			i += 1
		elseif c == "|" then
			return nil, locale.s("ds.code_editor.pattern.no_alternation")
		elseif c == "^" then
			table.insert(out, "%f[^\n]")
			i += 1
		elseif c == "$" then
			table.insert(out, "%f[\n\0]")
			i += 1
		else
			atom(escape(c))
			i += 1
		end
	end
	return table.concat(out), nil
end

-- A search: what to look for, how, and the pattern it makes (nil for a
-- plain search, which the engine runs where the text is held).
type Spec = { query: string, case: boolean, word: boolean, regex: boolean }
-- First and last byte (from 1), then the pattern's captures, if any.
type Match = { [number]: number, caps: { number | string }? }

local function pattern_of(spec: Spec): (string?, string?)
	if spec.regex then
		local p, err = translate(spec.query)
		if not p then
			return nil, err
		end
		if spec.word then
			p = "%f[%w_]" .. p .. "%f[^%w_]"
		end
		return if spec.case then p else fold(p), nil
	end
	local p = escape(spec.query)
	if spec.word then
		if is_word(string.sub(spec.query, 1, 1)) then
			p = "%f[%w_]" .. p
		end
		if is_word(string.sub(spec.query, -1)) then
			p = p .. "%f[^%w_]"
		end
	end
	return if spec.case then p else fold(p), nil
end

-- Every match of `pattern` in `t` (at most LIMIT), and whether there were
-- more. Searched in "\n" .. t so ^ finds the first line too; empty matches
-- are passed over.
local function find_all(t: string, pattern: string): ({ Match }, boolean)
	local s = "\n" .. t
	local out: { Match } = {}
	local at = 2
	local ok = pcall(function()
		while #out < LIMIT do
			local found = table.pack(string.find(s, pattern, at))
			local a, b = found[1], found[2]
			if type(a) ~= "number" or type(b) ~= "number" then
				return
			end
			if b < a then
				at = a + 1
			else
				local m: Match = { a - 1, b - 1 }
				if found.n > 2 then
					local caps: { number | string } = {}
					for k = 3, found.n do
						local cap = found[k]
						if cap ~= nil then
							table.insert(caps, cap)
						end
					end
					m.caps = caps
				end
				table.insert(out, m)
				at = b + 1
			end
			if at > #s then
				return
			end
		end
	end)
	if not ok then
		return {}, false
	end
	return out, #out >= LIMIT
end

-- The first match at or after offset `p` (its index from 1), wrapping.
local function index_after(list: { Match }, p: number): number
	for i, m in list do
		if m[1] > p then
			return i
		end
	end
	return 1
end

-- Brackets and their pairs.
local OPENERS: { [string]: string } = { ["("] = ")", ["["] = "]", ["{"] = "}" }
local CLOSERS: { [string]: string } = { [")"] = "(", ["]"] = "[", ["}"] = "{" }

-- The bracket's pair for the bracket at byte `i` (from 1), or nil.
local function pair_of(t: string, i: number): number?
	local c = string.sub(t, i, i)
	local close = OPENERS[c]
	if close then
		local class = "[%" .. c .. "%" .. close .. "]"
		local depth, at = 0, i
		local stop = i + BRACKET_REACH
		while true do
			local j = string.find(t, class, at, false)
			if not j or j > stop then
				return nil
			end
			depth += if string.sub(t, j, j) == c then 1 else -1
			if depth == 0 then
				return j
			end
			at = j + 1
		end
	end
	local open = CLOSERS[c]
	if open then
		-- Backward: the reach before it, reversed, searched forward (in C).
		local stop = math.max(1, i - BRACKET_REACH)
		local back = string.reverse(string.sub(t, stop, i))
		local class = "[%" .. c .. "%" .. open .. "]"
		local depth, at = 0, 1
		while true do
			local j = string.find(back, class, at, false)
			if not j then
				return nil
			end
			depth += if string.sub(back, j, j) == c then 1 else -1
			if depth == 0 then
				return i - j + 1
			end
			at = j + 1
		end
	end
	return nil
end

-- A line that opens a block, so the next one indents: an opener at its end,
-- or the language's own words for it.
local function opens_block(head: string, language: string?): boolean
	local last = string.match(head, "(%S)%s*$")
	if last and OPENERS[last] then
		return true
	end
	if language == "lua" or language == "luau" then
		return string.find(head, "%f[%w_]then%s*$") ~= nil
			or string.find(head, "%f[%w_]do%s*$") ~= nil
			or string.find(head, "%f[%w_]else%s*$") ~= nil
			or string.find(head, "%f[%w_]repeat%s*$") ~= nil
			or string.find(head, "%f[%w_]function%s*[%w_%.:]*%b()%s*$") ~= nil
	elseif language == "python" or language == "yaml" then
		return last == ":"
	elseif language == "shell" or language == "ruby" then
		return string.find(head, "%f[%w_]then%s*$") ~= nil or string.find(head, "%f[%w_]do%s*$") ~= nil
	end
	return false
end

---------------------------------------------------------------------------
-- The bars' small parts.

-- The find bar's controls: its toggles (Aa, whole word, pattern) and
-- glyphs (previous, next, close) are the design system's, small, their
-- tooltips below them so they never cover the line over the bar.
-- (`key`: the name's message; a tooltip is a string, read as it's built.)
local function toggle(key: string, glyph: string, on: State<boolean>): Node
	return ToggleButton { on = on, label = locale.t(key) :: any, tooltip = locale.s(key), text = glyph, size = "sm", tooltip_side = "bottom" }
end

local function glyph_button(key: string, icon: Icon.Name | () -> Icon.Name, run: () -> (), disabled: (() -> boolean)?): Node
	local button: IconButtonModule.Props = { icon = icon, label = locale.t(key), tooltip = locale.s(key), size = "sm", tooltip_side = "bottom", disabled = disabled, on_click = run }
	return IconButton(button)
end

---------------------------------------------------------------------------

return ui.component(function(props: Props): Node
	local value = props.value
	local readonly = props.readonly == true
	local numbered = props.line_numbers ~= false
	local folding = props.folding ~= false
	local show_status = if props.status == nil then not readonly else props.status
	local editor = ui.ref()
	local find_input = ui.ref()
	local goto_input = ui.ref()
	local scroller = ui.ref()
	-- A row's height: the window, the gutter and the blocks all stand on it.
	local SIZE, LINE = metrics(props.text_size)

	-- (States aren't plain tables: anything but a boolean is one.)
	local wrap: State<boolean> = if props.wrap ~= nil and type(props.wrap) ~= "boolean"
		then props.wrap :: State<boolean>
		else ui.use_state(props.wrap == true)

	local function language(): string?
		local l = props.language
		if l == nil or type(l) == "string" then
			return l :: string?
		elseif type(l) == "function" then
			return l()
		end
		return (l :: State<string?>):get()
	end

	---------------------------------------------------------------------------
	-- The file and what shows of it.

	local view: View.CodeView = View.new(value:peek())
	-- What the textarea holds; the scroll view's place and size; where the
	-- window stands in the rows; and a count each time the window is made,
	-- which the gutter and the marks follow.
	local shown = ui.use_state("")
	-- (Where it stood, if the caller kept it.)
	local start: Spot? = if props.place then props.place.get() else nil
	-- That place, held until the scroll view can go there (below).
	local holding: number? = if start and start.offset > 0 then start.offset else nil
	local offset = ui.use_state(if start then math.max(0, start.offset) else 0)
	local room = ui.use_state({ w = 0, h = 0 })
	-- The scroll view's element, kept from its `on_resize` (that handle
	-- reports a live box): a block opening in it brings itself into view
	-- by it.
	local viewport_el: any = nil
	local placed = ui.use_state({ row = 0, count = 1, total = 1 })
	local built = ui.use_state(0)
	-- A digit's width in the code's font, measured as the gutter first draws.
	local digit = ui.use_state(SIZE * 0.6)

	-- The code's sideways scroll, ours: the textarea is laid as wide as the
	-- widest line it holds, so it never scrolls sideways itself (a focused
	-- one scrolls back to its caret each time it draws, undoing a swipe),
	-- and slides in its clip by `hx`, which a swipe and the caret both move.
	-- A new window of lines keeps it.
	local hx = ui.use_state(0)

	-- The selection, in the file. It can run past the window, so the textarea
	-- holds only the part that shows; `pushed_a` and `pushed_f` are what it
	-- was last told, and while it still says so, the file's ends stand.
	local sel_a, sel_f = 0, 0
	if start then
		sel_a = math.clamp(start.anchor, 0, #view.text)
		sel_f = math.clamp(start.focus, 0, #view.text)
	end
	local pushed_a, pushed_f = 0, 0
	-- Where the caret is, as the status bar and the marks read it (the file's
	-- line and column), and the pair of brackets round it.
	local caret = ui.use_state({ line = 1, column = 1, anchor = 0, focus = 0, x = 0 })
	local pair = ui.use_state(nil :: { number }?)
	-- The column the caret keeps as it steps past a block, in bytes.
	local goal = 0
	-- A press is going on in the text: a caret landing in a fold's summary
	-- came from the pointer, and opens it.
	local pressing = false
	-- The textarea is up, and told where the selection is.
	local ready = false
	-- New text is on its way to the textarea, and the selection after it.
	local moved, settling = false, false

	-- Counted once an edit; what the find bar, the dirty dot and the bracket
	-- pair recompute from.
	local revision = ui.use_state(0)
	-- The saved text (held here, a state would copy it to Rust and back each
	-- save), and whether the text differs from it.
	local saved_state = props.saved
	local saved = if saved_state then saved_state:peek() else view.text
	local dirty = props.dirty or ui.use_state(false)
	local indent = ui.use_state(detect_indent(view.text))

	local function el(): Element?
		return editor:peek()
	end

	-- The window around the view, now. Whether the textarea was given new
	-- text; a caller then says where the selection is (`push`).
	local function build(): boolean
		local h = room:peek().h
		if h <= 0 then
			h = 800
		end
		local first = math.floor(offset:peek() / LINE)
		local last = first + math.ceil(h / LINE)
		local win = view:window(math.max(0, first - OVERSCAN), last + OVERSCAN)
		local changed = win.text ~= shown:peek()
		moved = moved or changed
		-- (The textarea can take the text, and say where its caret went,
		-- before this returns.)
		local was = settling
		settling = true
		ui.batch(function()
			if changed then
				shown:set(win.text)
			end
			placed:set({ row = win.first_row, count = #win.rows, total = view:total_rows() })
			built:set(built:peek() + 1)
		end)
		settling = was
		return changed
	end

	-- The text handed over and the selection put after it, now: a key
	-- mustn't land in the window before this one.
	local function settle_now()
		if not settling then
			return
		end
		local x = el()
		if x then
			pushed_a = view:to_window_clamped(sel_a)
			pushed_f = view:to_window_clamped(sel_f)
			x:set_selection_bytes(pushed_a, pushed_f)
		end
		settling, moved = false, false
	end

	-- Tells the textarea where the selection is, as much of it as it holds.
	-- New text reaches it after this turn and puts its caret at the end, so
	-- then the selection follows the text, and what it says until then isn't
	-- the person's doing (`settling`).
	local function push()
		local a = view:to_window_clamped(sel_a)
		local f = view:to_window_clamped(sel_f)
		pushed_a, pushed_f = a, f
		-- (Reading the ref hands the text over, so this comes first.)
		if moved and not settling then
			settling = true
			ui.after(0, settle_now)
		end
		local e = el()
		if e then
			e:set_selection_bytes(a, f)
		end
	end

	local function tell_caret()
		local line = view:line_of(sel_f)
		local head = string.sub(view.text, view:line_start(line) + 1, sel_f)
		caret:set { line = line, column = (utf8.len(head) or #head) + 1, anchor = sel_a, focus = sel_f, x = display_cols(head) }
	end

	-- The row a file offset shows on.
	local function row_of(p: number): number
		local line = view:shown_line(view:line_of(p))
		local row = view:row_of(line)
		local b = view.breaks[line]
		if b then
			local rel = p - view:line_start(line)
			for _, at in b do
				if rel >= at then
					row += 1
				end
			end
		end
		return row
	end

	-- (`reconcile`, below: an edit is in the file before the window moves.)
	local reconcile: () -> boolean

	-- The window, moved if the view has come near its end.
	local function slide()
		local win = view.win
		if not win then
			return
		end
		reconcile()
		local h = room:peek().h
		local first = math.floor(offset:peek() / LINE)
		local last = first + math.ceil(h / LINE)
		local from, to = win.first_row, win.first_row + #win.rows
		if (from > 0 and first - MARGIN < from) or (to < view:total_rows() and last + MARGIN > to) then
			build()
			push()
		end
	end

	-- The caret in view: the least scrolling, or in the middle.
	local function follow(middle: boolean?)
		local h = room:peek().h
		if h <= 0 then
			return
		end
		local y = row_of(sel_f) * LINE
		local o = offset:peek()
		local to = o
		if middle then
			to = y - (h - LINE) / 2
		elseif y < o then
			to = y
		elseif y + LINE > o + h then
			to = y + LINE - h
		end
		local limit = math.max(0, (view:total_rows() + TAIL) * LINE - h)
		to = math.clamp(to, 0, limit)
		if to ~= o then
			offset:set(to)
			slide()
		end
	end

	-- A new selection, in the file: shown, whatever had folded it away.
	local function place(a: number, f: number?, middle: boolean?)
		sel_a, sel_f = a, f or a
		goal = sel_f - view:line_start(view:line_of(sel_f))
		local lo, hi = math.min(sel_a, sel_f), math.max(sel_a, sel_f)
		if view:open_around(view:line_of(lo), view:line_of(hi)) then
			view:layout()
			build()
		end
		follow(middle)
		push()
		tell_caret()
	end

	-- The regions found again, once typing stops; and what the highlighter
	-- carries from line to line, for windows opened further down.
	local chain: { [number]: number } = {}
	local chain_to = 0
	local function chain_on()
		local base = Syntax.highlighter(language())
		if not base then
			return
		end
		local from = chain_to + 1
		if from > view.n then
			return
		end
		-- (Picks up where it stopped, a little each frame: from the last line
		-- remembered, or the top.)
		local s = chain[from - 1] or 0
		local stop = os.clock() + CHAIN_SECONDS
		local line = from
		while line <= view.n do
			local _, out = base(view:line_text(line), s)
			s = out :: number
			if line % CHAIN_STEP == 0 then
				chain[line] = s
				chain_to = line
				if os.clock() > stop then
					ui.after(CHAIN_REST, chain_on)
					return
				end
			end
			line += 1
		end
		chain_to = view.n
	end
	local function chain_from(line: number)
		chain_to = math.min(chain_to, math.max(0, (line - 1) - (line - 1) % CHAIN_STEP))
		for at in chain do
			if at > chain_to then
				chain[at] = nil
			end
		end
	end
	-- What the highlighter carries into `line`.
	local function state_before(line: number): number
		local base = Syntax.highlighter(language())
		local at = (line - 1) - (line - 1) % CHAIN_STEP
		if not base or at == 0 and line == 1 then
			return 0
		end
		local s = chain[at]
		if at > 0 and s == nil then
			return 0
		end
		s = s or 0
		for i = at + 1, line - 1 do
			local _, out = base(view:line_text(i), s)
			s = out :: number
		end
		return s
	end

	-- A scan gives way between frames, and starts over if the text moves on.
	local rescan_v = 0
	local function scan()
		if not folding then
			return
		end
		rescan_v += 1
		local v = rescan_v
		local stop = 0
		local regions: { Folds.Region }? = nil
		local run = coroutine.wrap(function()
			regions = Folds.scan(view.text, language(), comment_of(language()).line, function()
				if os.clock() > stop then
					coroutine.yield()
				end
			end)
		end)
		local function step()
			if v ~= rescan_v then
				return
			end
			stop = os.clock() + SCAN_SECONDS
			run()
			if not regions then
				ui.after(SCAN_REST, step)
				return
			end
			local changed = view:set_regions(regions)
			if changed then
				view:layout()
			end
			build()
			push()
		end
		step()
	end
	local function rescan()
		rescan_v += 1
		local v = rescan_v
		ui.after(RESCAN, function()
			if v == rescan_v then
				scan()
				chain_on()
			end
		end)
	end

	-- The text written out: what reads `value` sees each edit.
	local published = false
	local function publish()
		published = true
		value:set(view.text)
	end

	-- After an edit: rows, window and selection made true again.
	local function edited(lines_changed: boolean, line: number)
		if lines_changed or view.cols ~= nil or next(view.folded) ~= nil then
			view:layout()
		end
		if lines_changed and props.on_block_moved then
			for key, b in view.blocks do
				if b.line ~= b.given then
					b.given = b.line
					props.on_block_moved(key, b.line)
				end
			end
		end
		chain_from(line)
		build()
		push()
		follow()
		tell_caret()
		publish()
		rescan()
	end

	-- History: the file's, not the textarea's, which sees windows come and go.
	type Step = { at: number, old: string, new: string, a0: number, f0: number, a1: number, f1: number, time: number }
	local past: { Step } = {}
	local future: { Step } = {}
	local function record(at: number, old: string, new: string, a0: number, f0: number, a1: number, f1: number)
		table.clear(future)
		local now = os.clock()
		local top = past[#past]
		if top and now - top.time < RUN_SECONDS then
			-- Typing goes on from where it stopped, up to a line or a space.
			if old == "" and top.old == "" and top.at + #top.new == at and not string.find(new, "[%s]") then
				top.new ..= new
				top.a1, top.f1, top.time = a1, f1, now
				return
			end
			-- Backspacing eats back through it.
			if new == "" and top.new == "" and at + #old == top.at and not string.find(old, "\n", 1, true) then
				top.at = at
				top.old = old .. top.old
				top.a1, top.f1, top.time = a1, f1, now
				return
			end
		end
		table.insert(past, { at = at, old = old, new = new, a0 = a0, f0 = f0, a1 = a1, f1 = f1, time = now })
		if #past > HISTORY then
			table.remove(past, 1)
		end
	end

	-- What the textarea changed by itself (typing, a paste, a deletion), taken
	-- into the file. Everything reads through this first, so an edit is in
	-- the file before anything looks at it.
	reconcile = function(): boolean
		local win = view.win
		if not win then
			return false
		end
		local now = shown:peek()
		local old = win.text
		if now == old then
			return false
		end
		-- What differs: the start they share (by blocks, then bytes), then the
		-- end.
		local p, n, m = 0, #old, #now
		local limit = math.min(n, m)
		while p + 256 <= limit and string.sub(old, p + 1, p + 256) == string.sub(now, p + 1, p + 256) do
			p += 256
		end
		while p < limit and string.byte(old, p + 1) == string.byte(now, p + 1) do
			p += 1
		end
		local s = 0
		while s < limit - p and string.byte(old, n - s) == string.byte(now, m - s) do
			s += 1
		end
		local from: number = view:to_file(p)
		local to: number = view:to_file(n - s, true)
		-- A selection running past the window goes with what the window lost.
		local lo, hi = math.min(sel_a, sel_f), math.max(sel_a, sel_f)
		if lo ~= hi and from <= hi and to >= lo and p <= math.min(pushed_a, pushed_f) and n - s >= math.max(pushed_a, pushed_f) then
			from, to = math.min(from, lo), math.max(to, hi)
		end
		local ins = string.sub(now, p + 1, m - s)
		local removed = string.sub(view.text, from + 1, to)
		local line = view:line_of(from)
		local after = from + #ins
		record(from, removed, ins, sel_a, sel_f, after, after)
		local lines_changed = view:splice(from, to, ins)
		sel_a, sel_f = after, after
		goal = sel_f - view:line_start(view:line_of(sel_f))
		edited(lines_changed, line)
		return true
	end

	ui.effect(function()
		shown:get()
		reconcile()
	end)

	-- One edit: bytes `from` to `to` of the file become `text` (one step of
	-- undo), then the selection `sa` to `sf`.
	local function apply(from: number, to: number, text: string, sa: number, sf: number?)
		reconcile()
		local removed = string.sub(view.text, from + 1, to)
		local line = view:line_of(from)
		record(from, removed, text, sel_a, sel_f, sa, sf or sa)
		local lines_changed = view:splice(from, to, text)
		sel_a, sel_f = sa, sf or sa
		goal = sel_f - view:line_start(view:line_of(sel_f))
		edited(lines_changed, line)
	end

	local function step_history(back: boolean)
		reconcile()
		local list = if back then past else future
		local s = table.remove(list)
		if not s then
			return
		end
		local from, gone, put = s.at, if back then s.new else s.old, if back then s.old else s.new
		local line = view:line_of(from)
		local changed = view:splice(from, from + #gone, put)
		sel_a, sel_f = if back then s.a0 else s.a1, if back then s.f0 else s.f1
		view:open_around(line, view:line_of(from + #put))
		table.insert(if back then future else past, s)
		view:layout()
		edited(changed, line)
	end

	-- The selection as the textarea has it now, taken in. Where it lands on
	-- what isn't the file it goes on past: over a block's room, keeping its
	-- column, the way it was going; off a fold's summary to the line's end or
	-- the next line (a press there opens the fold). Between two rows of a
	-- wrapped line, the two ends of the break are one place: it steps on.
	local function absorb(a: number, f: number)
		local win = view.win
		if not win then
			return
		end
		local was_f = pushed_f
		local prior = sel_f
		local na = if a == pushed_a then sel_a else view:to_file(a)
		local nf = if f == pushed_f then sel_f else view:to_file(f)
		local rows = win.rows
		local k = View.row_at(win, f)
		local r = rows[k]
		local col = f - win.at[k]
		local again = false
		if r.spacer then
			local by = if f >= was_f then 1 else -1
			local j = k
			while rows[j] and rows[j].spacer do
				j += by
			end
			local t = rows[j]
			if t then
				nf = t.from + math.min(goal, t.to - t.from)
			else
				nf = prior
			end
			again = true
		elseif r.extra > 0 and col > r.to - r.from then
			again = true
			if pressing then
				view:unfold(r.line)
				view:layout()
				nf = r.to
				na = nf
				build()
			else
				local right = prior == r.to and f > was_f and f - was_f <= 4
				if right then
					local j = k + 1
					while rows[j] and rows[j].spacer do
						j += 1
					end
					nf = if rows[j] then rows[j].from else r.to
				else
					nf = r.to
				end
			end
		elseif r.more or (rows[k + 1] and rows[k + 1].more) then
			-- (One byte of what shows moved, none of the file.)
			if nf == prior and f ~= was_f and math.abs(f - was_f) == 1 and a == f then
				if f > was_f then
					nf = math.min(#view.text, prior + 1)
				else
					nf = math.max(0, prior - 1)
				end
				na = nf
				again = true
			end
		end
		if a == f and again then
			na = nf
		end
		sel_a, sel_f = na, nf
		if not r.spacer then
			goal = nf - view:line_start(view:line_of(nf))
		end
		if again then
			push()
		else
			pushed_a, pushed_f = a, f
		end
		tell_caret()
	end

	local function selection(): (number, number)
		reconcile()
		local e = el()
		if e and not settling then
			local a, f = e:selection_bytes()
			if a and f and (a ~= pushed_a or f ~= pushed_f) then
				absorb(a, f)
			end
		end
		return sel_a, sel_f
	end

	-- Folds and the line under the caret: asked for by keys, menus and the
	-- gutter.
	local function refold(keep: number?)
		-- The line at the top of the view stays there.
		local top_line = keep or view:line_at(math.floor(offset:peek() / LINE))
		local into = offset:peek() - view:row_of(top_line) * LINE
		view:layout()
		local to = view:row_of(view:shown_line(top_line)) * LINE + into
		if to ~= offset:peek() then
			offset:set(math.max(0, to))
		end
		build()
		-- A selection in what folded moves to the head that shows it.
		push()
		tell_caret()
	end
	-- Where a caret goes when a fold takes its line: the head line's text,
	-- its start, so a narrow pane doesn't scroll sideways to a line's end.
	local function head_of(line: number): number
		local lead = string.match(view:line_text(line), "^[ \t]*") :: string
		return view:line_start(line) + #lead
	end
	local function toggle_fold(line: number)
		if view:is_folded(line) then
			view:unfold(line)
		else
			view:fold(line)
		end
		refold(line)
	end
	local function fold_here(open: boolean)
		local _, f = selection()
		local line = view:line_of(f)
		if open then
			local shown_line = view:shown_line(line)
			if view:is_folded(shown_line) then
				view:unfold(shown_line)
				refold(shown_line)
			elseif view:is_folded(line) then
				view:unfold(line)
				refold(line)
			end
			return
		end
		local r = view:region_at(line)
		-- Already folded: the region round it, as each press goes one out.
		while r and view:is_folded(r.first) and r.first > 1 do
			r = view:region_at(r.first - 1)
		end
		if r and not view:is_folded(r.first) then
			view:fold(r.first)
			-- The caret was inside: it comes up to the line that shows.
			if line ~= r.first then
				sel_a, sel_f = head_of(r.first), head_of(r.first)
			end
			refold(r.first)
		end
	end
	local function fold_all()
		selection()
		view:fold_all()
		view:layout()
		local line = view:shown_line(view:line_of(sel_f))
		if line ~= view:line_of(sel_f) then
			sel_a, sel_f = head_of(line), head_of(line)
		end
		offset:set(0)
		refold(1)
		follow()
	end
	local function unfold_all()
		selection()
		if view:unfold_all() then
			refold()
			follow()
		end
	end

	-- A text put in from outside (another file, the other side of a split)
	-- is a new start: saved, its indent judged again, the history gone. Where
	-- most of it matches, what stands on it stays where it is.
	ui.effect(function()
		-- (Told without reading it: reading a long text copies it, and its
		-- own edit needn't be read back.)
		value:len()
		local own = published
		published = false
		local t = if own then view.text else value:peek()
		if t ~= view.text then
			local cur = view.text
			local p, n, m = 0, #cur, #t
			local limit = math.min(n, m)
			while p + 4096 <= limit and string.sub(cur, p + 1, p + 4096) == string.sub(t, p + 1, p + 4096) do
				p += 4096
			end
			while p < limit and string.byte(cur, p + 1) == string.byte(t, p + 1) do
				p += 1
			end
			local s = 0
			while s + 4096 <= limit - p and string.sub(cur, n - s - 4095, n - s) == string.sub(t, m - s - 4095, m - s) do
				s += 4096
			end
			while s < limit - p and string.byte(cur, n - s) == string.byte(t, m - s) do
				s += 1
			end
			local line = view:line_of(p)
			local changed = view:splice(p, n - s, string.sub(t, p + 1, m - s))
			if changed or view.cols ~= nil or next(view.folded) ~= nil then
				view:layout()
			end
			chain_from(line)
			sel_a = math.min(sel_a, #t)
			sel_f = math.min(sel_f, #t)
			table.clear(past)
			table.clear(future)
			if not readonly then
				saved = t
				indent:set(detect_indent(t))
			end
			build()
			push()
			rescan()
		end
		if saved_state then
			saved = saved_state:get()
		end
		ui.batch(function()
			if not readonly then
				dirty:set(t ~= saved)
			end
			revision:set(revision:peek() + 1)
		end)
	end)

	-- Where it stands, told to the caller that keeps it: a caret at once,
	-- a scroll a beat after it rests (no state written a frame), and as it
	-- goes.
	if props.place then
		local place = props.place
		-- Made again at a place kept from before: the scroll view can't go
		-- there until its content is laid out, and settles at the top. So
		-- the place is held, gone back to as the layout comes (and said as
		-- where it stands meanwhile), until it sticks or it's scrolled by
		-- hand (or a long while passes).
		if holding then
			ui.after(10000, function()
				holding = nil
			end)
			ui.effect(function()
				local r = room:get()
				built:get()
				local want = holding
				if not want or r.h <= 0 then
					return
				end
				if math.abs(offset:peek() - want) < 0.5 then
					holding = nil
					return
				end
				local most = math.max(0, (view:total_rows() + TAIL) * LINE - r.h)
				offset:set(math.min(want, most))
				-- (A new window puts the textarea's caret at its end: the
				-- selection is told again, or that caret is taken for the
				-- person's and the view follows it there, ~40 lines down.)
				if build() then
					push()
				end
			end)
		end
		local function tell()
			place.set({ offset = holding or offset:peek(), anchor = sel_a, focus = sel_f })
		end
		-- (The caret kept from before, said where the status bar reads it.)
		if start then
			tell_caret()
		end
		-- A caret moved is told at once; a scroll, once it rests a moment
		-- (not every frame of one), and the last of it as this goes. (An
		-- editor made again within that moment, as a reload mid-scroll,
		-- goes back to where the scroll last rested.)
		local told_offset, told_a, told_f = -1, -1, -1
		ui.effect(function()
			caret:get()
			if sel_a ~= told_a or sel_f ~= told_f then
				told_offset, told_a, told_f = holding or offset:peek(), sel_a, sel_f
				tell()
			end
		end)
		ui.effect(function()
			local o = offset:get()
			-- (This run's: the next frame of a scroll starts it over.)
			ui.after(120, function()
				local now = holding or o
				if now ~= told_offset then
					told_offset = now
					tell()
				end
			end)
		end)
		ui.on_cleanup(tell)
	end

	-- The first window, before anything has laid out; the regions a moment
	-- later, off the first frame. Wrapping's width follows the room there is.
	build()
	ui.after(0, function()
		scan()
		chain_on()
	end)

	-- The gutter's width: the + or an inset, the numbers, the fold control and
	-- the gap to the text.
	local lead = if props.on_add_block then Inline.SLOT else (if numbered then GUTTER_INSET else 0)
	local function gutter_w(): number
		placed:get()
		local digits = #tostring(math.max(view.n, 99))
		local numbers = if numbered then digits * digit:get() + lead else lead
		return numbers + (if folding then FOLD_SLOT + NUMBER_GAP else 8)
	end

	-- The text's room beside the gutter, and how wide the textarea is laid:
	-- the widest line it holds (with room past it for the caret), or that
	-- room if wider. Wrapped, the room: nothing runs past it.
	local function text_room(): number
		return math.max(0, room:get().w - gutter_w())
	end
	-- The widest line, in columns: all of the window's measured as the
	-- window moves; as it's typed in, only the caret's line at once (a
	-- line typed past the widest), the rest once the typing pauses (one
	-- that got shorter), not every line on every key.
	local wide_cols = ui.use_state(0)
	local wide_row: number? = nil
	ui.effect(function()
		local t = shown:get()
		local row = placed:peek().row
		if wide_row ~= row then
			wide_row = row
			wide_cols:set(widest(t))
			return
		end
		local from = line_start(view.text, sel_f)
		local cols = display_cols(string.sub(view.text, from + 1, line_end(view.text, sel_f)))
		if cols > wide_cols:peek() then
			wide_cols:set(cols)
		end
		-- (This run's: another key starts it over.)
		ui.after(250, function()
			wide_cols:set(widest(shown:peek()))
		end)
	end)
	local wide = ui.computed(function(): number
		if wrap:get() then
			return 0
		end
		return wide_cols:get() * digit:get() + 4 * digit:get()
	end)
	local function laid_w(): number
		return math.max(text_room(), wide:get())
	end
	-- (Kept within what there is to scroll to.)
	ui.effect(function()
		local most = math.max(0, wide:get() - text_room())
		if hx:peek() > most then
			hx:set(most)
		end
	end)
	-- The caret in view sideways, as it moves (not as the code redraws): a
	-- caret at a long line's end scrolls to it, Home back to the start.
	-- (Once per move: the room it's measured in changes as the window of
	-- lines moves, and the code mustn't go back to a caret swiped away from.)
	local followed = ""
	ui.effect(function()
		local c = caret:get()
		local d = digit:peek()
		local vw = text_room()
		if wrap:peek() or vw <= 0 then
			return
		end
		local at = `{c.anchor}:{c.focus}:{c.x}`
		if at == followed then
			return
		end
		followed = at
		local cx = (c.x or 0) * d
		local margin = 2 * d
		local h = hx:peek()
		local to = h
		if cx < h + margin then
			to = math.max(0, cx - margin)
		elseif cx > h + vw - margin then
			to = cx - vw + margin
		end
		to = math.clamp(to, 0, math.max(0, wide:peek() - vw))
		if to ~= h then
			hx:set(to)
		end
	end)

	ui.effect(function()
		local r = room:get()
		local on = wrap:get()
		local d = digit:get()
		local cols = nil
		if on and r.w > 0 then
			-- (Read outside the placing state it would follow.)
			local digits = #tostring(math.max(view.n, 99))
			local numbers = if numbered then digits * d + lead else lead
			local text_w = r.w - numbers - (if folding then FOLD_SLOT + NUMBER_GAP else 8) - 12
			cols = math.max(8, math.floor(text_w / d))
		end
		if cols ~= view.cols then
			selection()
			view.cols = cols
			view:layout()
			build()
			push()
		else
			slide()
		end
	end)
	ui.effect(function()
		offset:get()
		slide()
	end)

	-- Blocks, from the list given: each keeps its line as edits move it, and
	-- takes the rows its height rounds up to.
	local function block_list(): { Block }
		local b = props.blocks
		if b == nil then
			return {}
		elseif type(b) == "function" then
			return b()
		end
		return (b :: State<{ Block }>):get()
	end
	local standing = ui.use_state({} :: { Block })
	ui.effect(function()
		local list = block_list()
		local seen: { [string]: boolean } = {}
		local changed = false
		for _, b in list do
			seen[b.key] = true
			local line = math.clamp(b.line, 1, view.n)
			local have = view.blocks[b.key]
			if not have then
				view.blocks[b.key] = { line = line, rows = 3, given = line }
				view:open_around(line)
				changed = true
			elseif have.given ~= line then
				have.line, have.given = line, line
				changed = true
			end
		end
		for key in view.blocks do
			if not seen[key] then
				view.blocks[key] = nil
				changed = true
			end
		end
		if changed then
			selection()
			view:layout()
			build()
			push()
		end
		standing:set(list)
	end)
	local function block_rows(key: string, height: number)
		local b = view.blocks[key]
		local rows = math.max(1, math.ceil(height / LINE))
		if b and b.rows ~= rows then
			b.rows = rows
			selection()
			view:layout()
			build()
			push()
		end
	end
	-- Where a block's room starts: under its line, and any block before it
	-- on that line.
	local function block_row(key: string): number
		built:get()
		local b = view.blocks[key]
		if not b then
			return 0
		end
		local line = view:shown_line(b.line)
		local row = view:row_of(line) + 1
		if view.breaks[line] then
			row += #view.breaks[line]
		end
		for _, other in standing:peek() do
			if other.key == key then
				break
			end
			local o = view.blocks[other.key]
			if o and o.line == b.line then
				row += o.rows
			end
		end
		return row
	end

	local function save()
		if readonly then
			return
		end
		reconcile()
		local t = view.text
		if props.on_save then
			props.on_save(t)
		end
		saved = t
		if saved_state then
			saved_state:set(t)
		end
		dirty:set(false)
	end

	-- The search: open, its query and replacement, its toggles, which match
	-- is current (from 1; 0 for none), and the replace row.
	local find_open = props.find_open or ui.use_state(false)
	local replacing = ui.use_state(false)
	local query = ui.use_state("")
	local replacement = ui.use_state("")
	local case = ui.use_state(false)
	local word = ui.use_state(false)
	local regex = ui.use_state(false)
	local current = ui.use_state(0)
	local goto_open = ui.use_state(false)
	local goto_text = ui.use_state("")
	-- The line gone to, washed: its row's colour while it fades.
	local flash = ui.use_state(0)

	local function spec(): Spec
		return { query = query:get(), case = case:get(), word = word:get(), regex = regex:get() }
	end
	-- Whether the engine searches the text (plain) or a pattern does, here.
	local plain = ui.computed(function(): boolean
		return not word:get() and not regex:get()
	end)
	local problem = ui.computed(function(): string?
		if not find_open:get() or query:get() == "" or plain:get() then
			return nil
		end
		local _, err = pattern_of(spec())
		return err
	end)

	-- Every match in the file, found again once typing settles. (The file's
	-- matches, not the window's: a fold hides nothing from find.)
	local found = ui.use_state({} :: { Match })
	local found_more = ui.use_state(false)
	local settle: (() -> ())? = nil
	local function find_now(s: Spec, is_plain: boolean): ({ Match }, boolean)
		if is_plain then
			local list = (value:find(s.query, { case = s.case, limit = LIMIT })) :: any
			return list, #list >= LIMIT
		end
		local p = pattern_of(s)
		if p then
			return find_all(view.text, p)
		end
		return {}, false
	end
	ui.effect(function()
		local open = find_open:get()
		local s = spec()
		local is_plain = plain:get()
		revision:get()
		if settle then
			settle()
			settle = nil
		end
		if not open or s.query == "" then
			if #found:peek() > 0 then
				found:set({})
			end
			return
		end
		settle = ui.after(if is_plain then 0 else SETTLE, function()
			settle = nil
			local list, more = find_now(s, is_plain)
			ui.batch(function()
				found:set(list)
				found_more:set(more)
			end)
		end)
	end)

	-- Every match now, for stepping through them and replacing.
	local function matches(): { Match }
		local q = query:peek()
		if q == "" or not find_open:peek() then
			return {}
		end
		reconcile()
		return (find_now({ query = q, case = case:peek(), word = word:peek(), regex = regex:peek() }, plain:peek()))
	end
	local count = ui.computed(function(): { n: number, more: boolean }
		local q = query:get()
		if q == "" or not find_open:get() then
			return { n = 0, more = false }
		end
		return { n = #found:get(), more = found_more:get() }
	end)

	-- Match `index` selected and shown.
	local function show_match(index: number, list: { Match }?)
		local all = list or matches()
		if #all == 0 then
			current:set(0)
			return
		end
		index = (index - 1) % #all + 1
		current:set(index)
		place(all[index][1] - 1, all[index][2], true)
	end
	local function step(by: number)
		local all = matches()
		local cur = current:peek()
		if cur > 0 and cur <= #all then
			show_match(cur + by, all)
		else
			local i = index_after(all, caret:peek().focus)
			show_match(if by < 0 then i - 1 else i, all)
		end
	end

	-- Typing a query goes to its first match after where the caret was.
	local origin = 0
	ui.effect(function()
		local open = find_open:get()
		local q = query:get()
		case:get()
		word:get()
		regex:get()
		if not open or q == "" then
			current:set(0)
			return
		end
		ui.after(if plain:peek() then 0 else SETTLE + 10, function()
			local all = matches()
			if #all == 0 then
				current:set(0)
			else
				show_match(index_after(all, origin), all)
			end
		end)
	end)

	local function open_find(with_replace: boolean)
		local a, f = selection()
		origin = math.min(a, f)
		-- A short selection on one line is what to look for.
		if a ~= f and math.abs(a - f) < 200 then
			local picked = string.sub(view.text, math.min(a, f) + 1, math.max(a, f))
			if not string.find(picked, "\n", 1, true) then
				query:set(picked)
			end
		end
		if with_replace and not readonly then
			replacing:set(true)
		end
		find_open:set(true)
		goto_open:set(false)
		local i = find_input:peek()
		if i then
			i:focus()
			i:select_all()
		end
	end
	-- However it opens (⌘F, the viewer's button, its menu), the keys go to the
	-- field; however it closes, back to the text.
	local find_was = false
	ui.effect(function()
		local open = find_open:get()
		if open == find_was then
			return
		end
		find_was = open
		ui.after(0, function()
			if open then
				local a, f = selection()
				origin = math.min(a, f)
				local i = find_input:peek()
				if i then
					i:focus()
					i:select_all()
				end
			else
				current:set(0)
				local e = el()
				if e then
					e:focus()
				end
			end
		end)
	end)
	local function close_bars()
		find_open:set(false)
		goto_open:set(false)
		current:set(0)
		local e = el()
		if e then
			e:focus()
		end
	end

	-- The replacement for match `m`: as written, or (a pattern's) with $1…$9
	-- its groups.
	local function replacement_for(m: Match): string
		local r = replacement:peek()
		if not regex:peek() then
			return r
		end
		return (string.gsub(r, "%$(%d)", function(d: string)
			local cap = if m.caps then m.caps[tonumber(d) :: number] else nil
			return if type(cap) == "string" then cap else ""
		end))
	end
	local function replace_one()
		if readonly then
			return
		end
		local all = matches()
		local cur = current:peek()
		if cur == 0 or cur > #all then
			step(1)
			return
		end
		local m = all[cur]
		local r = replacement_for(m)
		apply(m[1] - 1, m[2], r, m[1] - 1 + #r)
		local after = matches()
		if #after > 0 then
			show_match(index_after(after, m[1] - 1 + #r - 1), after)
		else
			current:set(0)
		end
	end
	local function replace_all()
		if readonly then
			return
		end
		local all = matches()
		if #all == 0 then
			return
		end
		local t = view.text
		local parts = table.create(#all * 2 + 1)
		local at = 1
		for _, m in all do
			table.insert(parts, string.sub(t, at, m[1] - 1))
			table.insert(parts, replacement_for(m))
			at = m[2] + 1
		end
		table.insert(parts, string.sub(t, at))
		local a = selection()
		apply(0, #t, table.concat(parts), math.min(a, #t))
		current:set(0)
	end

	local function go_to(line: number, column: number?)
		line = math.clamp(line, 1, view.n)
		local at = view:line_start(line)
		local target = math.min(at + math.max(0, (column or 1) - 1), view:line_end(line))
		place(target, target, true)
		local e = el()
		if e then
			e:focus()
		end
		-- The row it lands on, washed, then fading.
		flash:set(2)
		ui.after(REVEAL_HOLD, function()
			flash:set(1)
			ui.after(REVEAL_FADE, function()
				flash:set(0)
			end)
		end)
	end
	if props.reveal then
		local reveal = props.reveal
		ui.effect(function()
			local at = reveal:get()
			if at then
				ui.after(0, function()
					if type(at) == "number" then
						go_to(at)
					elseif type(at) == "table" and type(at.line) == "number" then
						go_to(at.line, at.column)
						local last = at.end_line
						if type(last) == "number" and last > at.line then
							local from = view:line_start(math.clamp(at.line, 1, view.n))
							place(from, view:line_end(math.clamp(last, 1, view.n)), true)
						end
					end
					reveal:set(nil)
				end)
			end
		end)
	end
	local function open_goto()
		find_open:set(false)
		goto_text:set("")
		goto_open:set(true)
		local i = goto_input:peek()
		if i then
			i:focus()
		end
	end
	if props.commands then
		local c = props.commands
		c.save = save
		c.fold_all = fold_all
		c.unfold_all = unfold_all
		c.go_to_line = open_goto
	end
	local function commit_goto()
		local text = goto_text:peek()
		local line, column = string.match(text, "^%s*(%d+)%s*[:,]?%s*(%d*)%s*$")
		goto_open:set(false)
		if line then
			go_to(tonumber(line) :: number, tonumber(column))
		else
			local e = el()
			if e then
				e:focus()
			end
		end
	end


	---------------------------------------------------------------------------
	-- The editing keys.

	local function unit(): string
		return indent:peek()
	end

	-- Enter: the line's indent carried on, a step more after an opener; an
	-- opener with its closer just after puts the closer on its own line.
	local function enter(): boolean
		local t = view.text
		local a, f = selection()
		local lo, hi = math.min(a, f), math.max(a, f)
		local s = line_start(t, lo)
		local head = string.sub(t, s + 1, lo)
		local lead = string.match(head, "^[ \t]*") :: string
		local blank = string.match(head, "^[ \t]*$") ~= nil
		-- Space after the caret goes with it only from inside code (the text
		-- carried down starts at the indent); in an indent it is the line's.
		local tail_ws = if blank then 0 else #(string.match(string.sub(t, hi + 1, hi + 256), "^[ \t]*") :: string)
		local after = string.sub(t, hi + tail_ws + 1, hi + tail_ws + 1)
		local before = string.match(head, "(%S)%s*$")
		-- Leaving a line of indent alone leaves it empty, not trailing spaces.
		local from = if blank then s else lo
		if opens_block(head, language()) then
			local inner = "\n" .. lead .. unit()
			if before and OPENERS[before] == after then
				apply(from, hi + tail_ws, inner .. "\n" .. lead, from + #inner)
			else
				apply(from, hi + tail_ws, inner, from + #inner)
			end
		else
			apply(from, hi + tail_ws, "\n" .. lead, from + 1 + #lead)
		end
		return true
	end

	-- Backspace in an indent of spaces: back to the step before.
	local function backspace(): boolean
		local a, f = selection()
		if a ~= f or a == 0 then
			return false
		end
		local t = view.text
		local s = line_start(t, a)
		local head = string.sub(t, s + 1, a)
		local u = unit()
		if head == "" or u == "\t" or not string.match(head, "^ +$") then
			return false
		end
		local w = #u
		local keep = math.floor((#head - 1) / w) * w
		apply(s + keep, a, "", s + keep)
		return true
	end

	-- ⌘/: the lines' comments, on or off (off if every line has one).
	local function toggle_comment(): boolean
		local t = view.text
		local a, f = selection()
		local lo, hi = math.min(a, f), math.max(a, f)
		local s, e = lines_of(t, lo, hi)
		local c = comment_of(language())
		local lines = string.split(string.sub(t, s + 1, e), "\n")
		local out = table.create(#lines)
		local delta_first = 0
		if c.line then
			local token = c.line :: string
			local pat = "^([ \t]*)" .. escape(token) .. " ?"
			local all, least = true, math.huge
			for _, line in lines do
				if string.find(line, "%S") then
					if not string.find(line, pat) then
						all = false
					end
					least = math.min(least, #(string.match(line, "^[ \t]*") :: string))
				end
			end
			if least == math.huge then
				least = 0
			end
			for i, line in lines do
				local new = line
				if all then
					new = string.gsub(line, pat, "%1", 1)
				elseif string.find(line, "%S") or #lines == 1 then
					local cut = math.min(least, #line)
					new = string.sub(line, 1, cut) .. token .. " " .. string.sub(line, cut + 1)
				end
				if i == 1 then
					delta_first = #new - #line
				end
				out[i] = new
			end
		elseif c.block then
			local open, close = c.block[1], c.block[2]
			local pat = "^([ \t]*)" .. escape(open) .. " ?(.-) ?" .. escape(close) .. "%s*$"
			local all = true
			for _, line in lines do
				if string.find(line, "%S") and not string.find(line, pat) then
					all = false
				end
			end
			for i, line in lines do
				local new = line
				if all then
					new = string.gsub(line, pat, "%1%2", 1)
				elseif string.find(line, "%S") then
					local lead, rest = string.match(line, "^([ \t]*)(.*)$")
					new = `{lead}{open} {rest} {close}`
				end
				if i == 1 then
					delta_first = #new - #line
				end
				out[i] = new
			end
		else
			return true
		end
		local text = table.concat(out, "\n")
		if lo == hi then
			apply(s, e, text, math.max(s, lo + delta_first))
		else
			apply(s, e, text, s, s + #text)
		end
		return true
	end

	-- ⌥↑ ⌥↓: the lines swap with the one above or below.
	local function move(down: boolean): boolean
		local t = view.text
		local a, f = selection()
		local s, e = lines_of(t, math.min(a, f), math.max(a, f))
		local block = string.sub(t, s + 1, e)
		if down then
			if e >= #t then
				return true
			end
			local next_e = line_end(t, e + 1)
			local below = string.sub(t, e + 2, next_e)
			local shift = #below + 1
			apply(s, next_e, below .. "\n" .. block, a + shift, f + shift)
		else
			if s == 0 then
				return true
			end
			local prev_s = line_start(t, s - 1)
			local above = string.sub(t, prev_s + 1, s - 1)
			local shift = #above + 1
			apply(prev_s, e, block .. "\n" .. above, a - shift, f - shift)
		end
		return true
	end

	-- ⇧⌥↑ ⇧⌥↓: the lines copied above or below, the caret on the copy.
	local function duplicate(down: boolean): boolean
		local t = view.text
		local a, f = selection()
		local s, e = lines_of(t, math.min(a, f), math.max(a, f))
		local block = string.sub(t, s + 1, e)
		if down then
			local shift = #block + 1
			apply(e, e, "\n" .. block, a + shift, f + shift)
		else
			apply(s, s, block .. "\n", a, f)
		end
		return true
	end

	-- ⇧⌘K: the lines gone.
	local function delete_lines(): boolean
		local t = view.text
		local a, f = selection()
		local s, e = lines_of(t, math.min(a, f), math.max(a, f))
		if e < #t then
			apply(s, e + 1, "", s)
		else
			local from = math.max(0, s - 1)
			apply(from, e, "", line_start(t, from))
		end
		return true
	end

	-- ⌘] ⌘[: the lines a step in or out.
	local function shift_lines(out_: boolean): boolean
		local t = view.text
		local a, f = selection()
		local lo, hi = math.min(a, f), math.max(a, f)
		local s, e = lines_of(t, lo, hi)
		local u = unit()
		local lines = string.split(string.sub(t, s + 1, e), "\n")
		local first = 0
		for i, line in lines do
			local new = line
			if out_ then
				if string.sub(line, 1, 1) == "\t" then
					new = string.sub(line, 2)
				else
					local lead = #(string.match(line, "^ *") :: string)
					new = string.sub(line, math.min(lead, #u) + 1)
				end
			elseif string.find(line, "%S") then
				new = u .. line
			end
			if i == 1 then
				first = #new - #line
			end
			lines[i] = new
		end
		local text = table.concat(lines, "\n")
		if lo == hi then
			apply(s, e, text, math.max(s, lo + first))
		else
			apply(s, e, text, s, s + #text)
		end
		return true
	end

	-- ⌘↵ ⇧⌘↵: a new line below or above, indented as this one.
	local function open_line(above: boolean): boolean
		local t = view.text
		local a, f = selection()
		local s, e = lines_of(t, math.min(a, f), math.max(a, f))
		local lead = string.match(string.sub(t, s + 1, e), "^[ \t]*") :: string
		if above then
			apply(s, s, lead .. "\n", s + #lead)
		else
			apply(e, e, "\n" .. lead, e + 1 + #lead)
		end
		return true
	end

	-- ⌘L: the line selected (again: the next one too).
	local function select_line(): boolean
		local t = view.text
		local a, f = selection()
		local s, e = lines_of(t, math.min(a, f), math.max(a, f))
		local e2 = if math.max(a, f) > e then line_end(t, math.max(a, f) + 1) else e
		local e_ = math.min(#t, e2 + 1)
		place(s, e_)
		return true
	end

	-- An opener typed: its closer after it (where only space or a closer
	-- follows), or round a selection. A closer typed before the same closer
	-- steps over it.
	local function bracket(c: string): boolean
		local t = view.text
		local a, f = selection()
		local lo, hi = math.min(a, f), math.max(a, f)
		local close = OPENERS[c]
		if close then
			if lo ~= hi then
				apply(lo, hi, c .. string.sub(t, lo + 1, hi) .. close, lo + 1, hi + 1)
				return true
			end
			local next_c = string.sub(t, hi + 1, hi + 1)
			if next_c == "" or string.match(next_c, "%s") or CLOSERS[next_c] then
				apply(lo, hi, c .. close, lo + 1)
				return true
			end
			return false
		end
		if CLOSERS[c] and lo == hi and string.sub(t, hi + 1, hi + 1) == c then
			place(hi + 1)
			return true
		end
		return false
	end

	-- The keys that would cross what isn't the file: Backspace at a row's
	-- start and Delete at its end, beside a fold, a block or a wrapped line's
	-- break. The textarea would take the room or the summary; the file loses
	-- the one character there, and a fold it reaches opens rather than going
	-- with it.
	local function delete_across(back: boolean): boolean
		local a, f = selection()
		local win = view.win
		if a ~= f or not win then
			return false
		end
		local w = view:to_window(f)
		if not w then
			return false
		end
		local rows, at = win.rows, win.at
		local k = View.row_at(win, w)
		local r = rows[k]
		local col = w - at[k]
		local t = view.text
		if back then
			if col ~= 0 or k == 1 or f == 0 then
				return false
			end
			local above = rows[k - 1]
			if not (r.more or above.spacer or above.extra > 0) then
				return false
			end
			local from = utf8.offset(t, -1, f + 1) or f
			from -= 1
			if view:open_around(view:line_of(from)) then
				view:layout()
			end
			apply(from, f, "", from)
			return true
		end
		local below = rows[k + 1]
		if col ~= r.to - r.from or not below or f >= #t then
			return false
		end
		if not (below.more or below.spacer or r.extra > 0) then
			return false
		end
		local nxt = utf8.offset(t, 2, f + 1) or (f + 2)
		if view:open_around(view:line_of(f)) then
			view:layout()
		end
		apply(f, nxt - 1, "", f)
		return true
	end

	local function copy_selection(): boolean
		local a, f = selection()
		if a ~= f then
			ui.clipboard.write(string.sub(view.text, math.min(a, f) + 1, math.max(a, f)))
		end
		return true
	end

	-- Page Up and Down, and the file's ends: the textarea would stop at the
	-- window's.
	local function page(rows: number, extend: boolean): boolean
		local _, f = selection()
		local row = math.clamp(row_of(f) + rows, 0, view:total_rows() - 1)
		local line = view:line_at(row)
		while line > 1 and view:is_hidden(line) do
			line -= 1
		end
		local col = f - view:line_start(view:line_of(f))
		local to = math.min(view:line_start(line) + col, view:line_end(line))
		offset:set(math.max(0, offset:peek() + rows * LINE))
		slide()
		place(if extend then sel_a else to, to)
		return true
	end
	local function jump(to: number, extend: boolean): boolean
		selection()
		place(if extend then sel_a else to, to)
		return true
	end

	-- ⌥⌘[ ⌥⌘]: fold and open round the caret; with ⇧, all of them. (By the
	-- key's place: ⌥ changes what it types.)
	local function fold_key(e: KeyEvent): boolean
		if not (folding and e.cmd and e.alt) then
			return false
		end
		local left = e.code == "BracketLeft" or e.key == "[" or e.key == "{"
		local right = e.code == "BracketRight" or e.key == "]" or e.key == "}"
		if not left and not right then
			return false
		end
		if e.shift then
			if left then
				fold_all()
			else
				unfold_all()
			end
		else
			fold_here(right)
		end
		return true
	end

	local function on_editor_key(e: KeyEvent): boolean?
		reconcile()
		settle_now()
		if fold_key(e) then
			return true
		end
		local k = string.lower(e.key)
		local cmd = e.cmd
		if cmd and not e.alt then
			if k == "c" and not e.shift then
				return copy_selection()
			elseif k == "a" and not e.shift then
				place(0, #view.text)
				return true
			elseif k == "arrowup" then
				return jump(0, e.shift)
			elseif k == "arrowdown" then
				return jump(#view.text, e.shift)
			elseif k == "l" and not e.shift then
				return select_line()
			end
		end
		if k == "pageup" or k == "pagedown" then
			local rows = math.max(1, math.floor(room:peek().h / LINE) - 1)
			return page(if k == "pageup" then -rows else rows, e.shift)
		end
		if readonly then
			return nil
		end
		-- Scrolled away, typing brings the caret back first.
		if not cmd and not e.ctrl and k ~= "shift" and k ~= "alt" and k ~= "meta" then
			follow()
			settle_now()
		end
		if e.ctrl and not cmd then
			return nil
		end
		if cmd then
			if k == "z" then
				step_history(not e.shift)
				return true
			elseif k == "x" and not e.shift then
				local a, f = selection()
				if a ~= f then
					copy_selection()
					local lo, hi = math.min(a, f), math.max(a, f)
					apply(lo, hi, "", lo)
				end
				return true
			elseif k == "/" and not e.shift then
				return toggle_comment()
			elseif k == "k" and e.shift then
				return delete_lines()
			elseif k == "]" and not e.alt then
				return shift_lines(false)
			elseif k == "[" and not e.alt then
				return shift_lines(true)
			elseif k == "enter" then
				return open_line(e.shift)
			end
			return nil
		end
		if e.alt then
			if k == "arrowup" or k == "arrowdown" then
				local down = k == "arrowdown"
				return if e.shift then duplicate(down) else move(down)
			end
			return nil
		end
		if k == "tab" then
			local a, f = selection()
			if a ~= f or e.shift then
				return shift_lines(e.shift)
			end
			return nil
		end
		if k == "enter" and not e.shift then
			return enter()
		elseif k == "backspace" and not e.shift then
			return delete_across(true) or backspace()
		elseif k == "delete" then
			return delete_across(false)
		elseif OPENERS[e.key] or CLOSERS[e.key] then
			return bracket(e.key)
		end
		return nil
	end

	-- Keys for the whole editor, bars included.
	local function on_frame_key(e: KeyEvent): boolean?
		local k = string.lower(e.key)
		if fold_key(e) then
			return true
		end
		if e.cmd and k == "f" then
			open_find(e.alt)
			return true
		elseif e.cmd and k == "g" then
			if find_open:peek() then
				step(if e.shift then -1 else 1)
			end
			return true
		elseif e.ctrl and k == "g" then
			open_goto()
			return true
		elseif e.cmd and k == "s" and not e.shift then
			save()
			return true
		elseif e.alt and k == "z" and not e.cmd then
			wrap:set(not wrap:peek())
			return true
		elseif k == "escape" and (find_open:peek() or goto_open:peek()) then
			close_bars()
			return true
		end
		return nil
	end

	---------------------------------------------------------------------------
	-- What is marked behind the text: the bracket pair, the matches and a
	-- line gone to.

	local function on_caret(_line: number, _column: number, anchor: number, focus: number)
		-- (A textarea comes up with its caret at its end; `on_mount` says where.)
		if not ready then
			return
		end
		reconcile()
		if settling or (anchor == pushed_a and focus == pushed_f) then
			return
		end
		absorb(anchor, focus)
		follow()
	end
	-- The bracket just after the caret, else just before it, and its pair.
	ui.effect(function()
		local c = caret:get()
		revision:get()
		local found_pair: { number }? = nil
		if c.anchor == c.focus then
			local t = view.text
			for _, i in { c.focus + 1, c.focus } do
				if i >= 1 then
					local ch = string.sub(t, i, i)
					if OPENERS[ch] or CLOSERS[ch] then
						local j = pair_of(t, i)
						if j then
							found_pair = { i, j }
						end
						break
					end
				end
			end
		end
		local old = pair:peek()
		if found_pair == nil and old == nil then
			return
		end
		if found_pair and old and found_pair[1] == old[1] and found_pair[2] == old[2] then
			return
		end
		pair:set(found_pair)
	end)

	-- Marks are the file's, the textarea's are the window's: only those that
	-- show are handed over.
	local decorations = ui.computed(function(): { { any } }
		built:get()
		local win = view.win
		local out: { { any } } = {}
		if not win then
			return out
		end
		local function mark(from: number, to: number, color: string)
			local a = view:to_window(from - 1)
			local b = view:to_window(to)
			if a and b and b > a then
				table.insert(out, { a + 1, b, color })
			end
		end
		local p = pair:get()
		if p then
			mark(p[1], p[1], "$zeron.editor.bracket")
			mark(p[2], p[2], "$zeron.editor.bracket")
		end
		if find_open:get() then
			local cur = current:get()
			local list = found:get()
			local lo = view:line_start(win.first_line)
			local hi = view:line_end(win.last_line)
			-- The first match that could show, by halves.
			local a, b = 1, #list + 1
			while a < b do
				local mid = (a + b) // 2
				if list[mid][2] <= lo then
					a = mid + 1
				else
					b = mid
				end
			end
			for i = a, #list do
				local m = list[i]
				if m[1] - 1 > hi then
					break
				end
				mark(m[1], m[2], if i == cur then "$zeron.editor.match_current" else "$zeron.editor.match")
			end
		end
		return out
	end)

	-- The caret's row: Zeron's faint wash while editing; none in a viewer,
	-- but for a line gone to (washed in the accent, then fading).
	local function row_color(): string
		local f = flash:get()
		if f == 2 then
			return "$zeron.editor.reveal"
		elseif f == 1 then
			return "$zeron.editor.current"
		end
		return if readonly then "#00000000" else "$zeron.editor.current"
	end

	---------------------------------------------------------------------------
	-- The pieces.

	-- A line's colours: the language's, for the file's text. The first line
	-- of a window starts with what the file carries there (0 is what the
	-- textarea says of a first line, so other states are kept one up). A
	-- fold's summary is coloured here, and a wrapped line is coloured whole
	-- and each row given its part.
	local function colours(lang: string?): (string, any) -> ({ { any } }, any)
		local base = Syntax.highlighter(lang)
			or function(_line: string, state: any): ({ { any } }, any)
				return {}, state
			end
		return function(line: string, state: any): ({ { any } }, any)
			local s = if state == 0 then state_before((view.win :: any).first_line) else (state :: number) - 1
			local win = view.win :: any
			local tail = win and win.summaries[line]
			if tail then
				local spans, out = base(string.sub(line, 1, tail), s)
				local list = table.clone(spans)
				local e = tail + #View.ELLIPSIS
				table.insert(list, { tail + 1, e, "$text.tertiary", bg = "$zeron.editor.fold" })
				table.insert(list, { e + 1, #line, "$text.tertiary" })
				return list, (out :: number) + 1
			end
			local part = win and win.wrapped[line]
			if part then
				local spans, out = base(part[1], s)
				local from, to = part[2], part[3]
				local list = {}
				for _, sp in spans do
					if sp[2] > from and sp[1] <= to then
						local c = table.clone(sp)
						c[1] = math.max(1, sp[1] - from)
						c[2] = math.min(to - from, sp[2] - from)
						table.insert(list, c)
					end
				end
				return list, (if part[4] then out :: number else s) + 1
			end
			local spans, out = base(line, s)
			return spans, (out :: number) + 1
		end
	end

	-- A selection dragged past the view's edge carries the view with it,
	-- faster the further the pointer goes; its end follows the row at the
	-- pointer's height. (The textarea's own carrying stops at its own edge,
	-- and it has none here: it never scrolls.)
	-- Past the code's left or right edge (unwrapped), it carries `hx` the
	-- same way, the selection's end at the column under the pointer.
	local drag_y: number? = nil
	local drag_x: number? = nil
	local dragging = false
	local function drag_step()
		local h = room:peek().h
		local vw = text_room()
		local y, x = drag_y, drag_x
		local past = if y == nil then 0 elseif y < 0 then y elseif y > h then y - h else 0
		local past_x = if x == nil or wrap:peek() then 0 elseif x < 0 then x elseif x > vw then x - vw else 0
		if not pressing or (past == 0 and past_x == 0) or y == nil then
			dragging = false
			return
		end
		if past ~= 0 then
			local limit = math.max(0, (view:total_rows() + TAIL) * LINE - h)
			offset:set(math.clamp(offset:peek() + math.clamp(past / 3, -DRAG_MAX, DRAG_MAX), 0, limit))
			slide()
		end
		if past_x ~= 0 then
			local most = math.max(0, wide:peek() - vw)
			hx:set(math.clamp(hx:peek() + math.clamp(past_x / 3, -DRAG_MAX, DRAG_MAX), 0, most))
		end
		local line = view:line_at(math.floor((offset:peek() + math.clamp(y, 0, h - 1)) / LINE))
		if past ~= 0 then
			sel_f = if past > 0 then view:line_end(line) else view:line_start(line)
		else
			-- (The column at the pointer, held to the code's edge.)
			local start = view:line_start(line)
			local text = string.sub(view.text, start + 1, view:line_end(line))
			local col = math.floor((math.clamp(x :: number, 0, vw) + hx:peek()) / digit:peek())
			sel_f = start + byte_at_col(text, col)
		end
		push()
		tell_caret()
		ui.after(DRAG_TICK, drag_step)
	end

	local function textarea(lang: string?): Node
		return ui.textarea(EditorMenu.on({
			id = if readonly then "code-viewer" else "code-editor",
			ref = editor,
			label = props.label or locale.t("ds.code_editor.code"),
			value = shown,
			read_only = readonly,
			wrap = false,
			highlight = colours(lang),
			font = "$font.mono",
			text_size = ui.raw(SIZE, "Settings' code font size, or the design system's"),
			line_height = ui.raw(LINE, "a row of the window, the gutter and the blocks, scaled with the code"),
			color = "$text.primary",
			w_full = true,
			h_full = true,
			-- No inset: a textarea clips inside its padding.
			pl = 0,
			pr = 0,
			py = 0,
			border = 0,
			rounded = 0,
			bg = "#00000000",
			focus = { border = 0, outline = 0 },
			caret_color = if readonly then "#00000000" else "$accent",
			caret_width = 2,
			current_line = row_color,
			tab = if readonly then nil else indent:peek(),
			-- (Tab indents here: how to leave by keys, said, as nothing on
			-- screen shows it. The caller decides where Escape goes.)
			description = if readonly then nil else props.escape_hint,
			decorations = decorations,
			on_caret = on_caret,
			on_key = on_editor_key,
			on_mount = function()
				ready = true
				push()
			end,
			-- The textarea scrolls its long lines sideways itself, and a gesture
			-- that moves them, however slightly, latches to it, so the rest of
			-- it would be lost to the scroll view around it. Over the text, the
			-- wheel's vertical half goes straight to that view. (`true`: it
			-- doesn't get it twice.)
			on_wheel = function(e): boolean?
				-- (Scrolled by hand: a place held from before is let go.)
				holding = nil
				if e.cmd or e.ctrl then
					return nil
				end
				-- Sideways: `hx`, the one offset the caret moves too, so a
				-- swipe brings back what a caret at a long line's end
				-- scrolled away, and the other way.
				local dx = e.dx or 0
				if math.abs(dx) > math.abs(e.dy) then
					local most = math.max(0, wide:peek() - text_room())
					hx:set(math.clamp(hx:peek() + dx, 0, most))
					return true
				end
				if e.dy ~= 0 then
					local limit = math.max(0, (view:total_rows() + TAIL) * LINE - room:peek().h)
					offset:set(math.clamp(offset:peek() + e.dy, 0, limit))
				end
				return true
			end,
			on_pointer_move = function(e)
				if pressing then
					local box = scroller:peek()
					drag_y = e.y - (if box then box.y or 0 else 0)
					-- (From the code's own left edge, past the gutter.)
					drag_x = e.x - (if box then box.x or 0 else 0) - gutter_w()
					local out_x = not wrap:peek() and (drag_x < 0 or drag_x > text_room())
					if not dragging and (drag_y < 0 or drag_y > room:peek().h or out_x) then
						dragging = true
						ui.after(DRAG_TICK, drag_step)
					end
				end
			end,
			-- Not the engine's own menu: ours, round it.
			context_menu = function(_e: PointerEvent): { Node }?
				return nil
			end,
		}))
	end

	-- Whether the pointer is in the gutter; which row of what shows it is on,
	-- anywhere over the code, the gutter or the + (-1: not over them); and
	-- that row's height in the view, for when the code scrolls under a pointer
	-- that stays put.
	local over = ui.use_state(false)
	local over_row = ui.use_state(-1)
	local pointer_y: number? = nil
	-- A press on the +: its row stands until the button is let go.
	local add_row: number? = nil
	local function point_at(y: number?)
		pointer_y = y
		if add_row then
			return
		end
		local row = if y then math.floor((y + offset:peek()) / LINE) else -1
		if row ~= over_row:peek() then
			over_row:set(row)
		end
	end
	ui.effect(function()
		offset:get()
		if pointer_y then
			point_at(pointer_y)
		end
	end)
	-- The row of the window the pointer is on, if it shows a line.
	local function pointed(): Row?
		built:get()
		local win = view.win
		local row = over_row:get()
		if not win or row < 0 then
			return nil
		end
		local r = win.rows[row - win.first_row + 1]
		return if r and not r.spacer then r else nil
	end

	-- The gutter. Numbers on a canvas beside the window, moving with it, the
	-- caret's brighter; a wrapped line numbered at its first row, a block's
	-- rows left empty.
	local function number_at(c: any, line: number, x: number, y: number, ink: string)
		c:text(tostring(line), x, y, {
			size = SIZE,
			font = "$font.mono",
			tabular = true,
			align = "end",
			baseline = "middle",
			color = ink,
		})
	end
	-- Where the numbers end, in a gutter `w` wide.
	local function numbers_end(w: number): number
		return w - (if folding then FOLD_SLOT + NUMBER_GAP else 8)
	end
	local function numbers(): Node
		return ui.canvas {
			absolute = true,
			left = 0,
			top = 0,
			w_full = true,
			h_full = true,
			pointer_events = "none",
			draw = function(c, w, _h)
				built:get()
				local win = view.win
				if not win then
					return
				end
				-- (In code's family by name: a canvas's font takes no token.
				-- Read here, a new code font measures again.)
				local measured = c:measure_text("0", { size = SIZE, font = look.code_family() })
				if measured and measured > 0 and math.abs(measured - digit:peek()) > 0.01 then
					ui.after(0, function()
						digit:set(measured)
					end)
				end
				local x = numbers_end(w)
				for k, r in win.rows do
					if not r.spacer and not r.more then
						number_at(c, r.line, x, (k - 1) * LINE + LINE / 2, "$text.tertiary")
					end
				end
			end,
		}
	end
	-- The caret's number, over the rest in the brighter ink: a canvas of its
	-- own, so a caret moving doesn't draw every number again.
	local function caret_number(): Node
		return ui.canvas {
			absolute = true,
			left = 0,
			top = 0,
			w_full = true,
			h_full = true,
			pointer_events = "none",
			draw = function(c, w, _h)
				built:get()
				local line = caret:get().line
				local win = view.win
				if not win or view:is_hidden(line) then
					return
				end
				local k = view:row_of(line) - win.first_row
				if k >= 0 and k < #win.rows then
					number_at(c, line, numbers_end(w), k * LINE + LINE / 2, "$text.secondary")
				end
			end,
		}
	end

	-- The fold controls, down the gutter's right side: each region's chevron
	-- while the pointer is in the gutter, a folded one always. One canvas, so
	-- a screen of them is one node. A press here folds.
	local function chevrons(): Node
		return ui.canvas {
			absolute = true,
			right = 0,
			top = 0,
			w = FOLD_SLOT,
			h_full = true,
			cursor = function(): Cursor
				local r = pointed()
				return if r and not r.more and view.by_first[r.line] then "pointer" else "default"
			end,
			on_press = function(e)
				local win = view.win
				if not win then
					return
				end
				local r = win.rows[math.floor(e.local_y / LINE) + 1]
				if r and not r.spacer and not r.more and view.by_first[r.line] then
					selection()
					toggle_fold(r.line)
				end
				-- (The press shouldn't take the keys away from the text.)
				local x = el()
				if x then
					x:focus()
				end
			end,
			draw = function(c, w, _h)
				built:get()
				local near = over:get()
				local win = view.win
				if not win then
					return
				end
				local under = over_row:get() - win.first_row + 1
				local cx = w / 2 - 1
				for k, r in win.rows do
					if not r.spacer and not r.more and view.by_first[r.line] then
						local folded = view:is_folded(r.line)
						if folded or near then
							local cy = (k - 1) * LINE + LINE / 2
							local ink = if folded or k == under then "$text.secondary" else "$text.tertiary"
							local path = if folded
								then ui.path():move(cx - 2, cy - 4):line(cx + 2, cy):line(cx - 2, cy + 4)
								else ui.path():move(cx - 4, cy - 2):line(cx, cy + 2):line(cx + 4, cy - 2)
							c:stroke(path, ink, 1.25, { cap = "round", join = "round" })
						end
					end
				end
			end,
		}
	end

	-- A theme changing recolours the textarea's text but not the lines it has
	-- already drawn (zgx: `EditorHandle::set_text_style` touches only the text
	-- node), so they keep the old ink until something draws them. A canvas
	-- hears of a theme where this subtree's own is one: a speck of a token
	-- that differs between themes, and the selection put back, draws them.
	local function theme_watch(): Node
		return ui.canvas {
			absolute = true,
			left = 0,
			top = 0,
			w = 1,
			h = 1,
			pointer_events = "none",
			draw = function(c)
				c:rect(0, 0, 1, 1, "$zeron.editor.current")
				ui.after(0, function()
					local e = el()
					if e and not settling then
						e:set_selection_bytes(pushed_a, pushed_f)
					end
				end)
			end,
		}
	end

	local function gutter(): Node
		return ui.div {
			absolute = true,
			left = 0,
			w = gutter_w,
			top = function(): number
				return placed:get().row * LINE
			end,
			h = function(): number
				return math.max(1, placed:get().count) * LINE
			end,
			cursor = "default",
			on_hover = function(inside: boolean)
				over:set(inside)
			end,
			if numbered then numbers() else nil,
			if numbered then caret_number() else nil,
			if folding then chevrons() else nil,
		}
	end

	-- Whether a block stands right under another, on the same line.
	local function joined(b: Block): boolean
		for _, other in standing:peek() do
			if other.key == b.key then
				return false
			elseif other.line == b.line then
				return true
			end
		end
		return false
	end

	-- A block over the room left for it: as wide as the pane (the code's
	-- sideways scroll never moves it), as tall as its rows.
	local function block_node(b: Block): Node
		return ui.div {
			absolute = true,
			left = 0,
			right = 0,
			top = function(): number
				return block_row(b.key) * LINE
			end,
			h = function(): number
				built:get()
				local have = view.blocks[b.key]
				return (if have then have.rows else 1) * LINE
			end,
			ui.show(function(): boolean
				built:get()
				local have = view.blocks[b.key]
				return have ~= nil and not view:is_hidden(have.line)
			end, ui.column {
				w_full = true,
				-- (Its own height, not the room it has so far: that is what this
				-- measures.)
				shrink = 0,
				on_resize = function(size)
					ui.after(0, function()
						block_rows(b.key, size.h)
					end)
				end,
				(props.block_view :: BlockView)(b, {
				inset = gutter_w,
				joined = joined(b),
				reveal = {
					offset = offset,
					frame = function()
						return viewport_el
					end,
				},
			}),
			}),
		}
	end

	-- The + in its slot, on the row the pointer is on, wherever along that row
	-- it is: one button that moves, so crossing lines builds nothing and it
	-- is there when the pointer arrives. A press holds it to its row.
	local function add_control(): Node?
		local add = props.on_add_block
		if not add then
			return nil
		end
		local box = (ui.theme.get("zeron.icon_button.sm") :: number?) or 22
		return ui.show(function(): boolean
			return pointed() ~= nil
		end, ui.div {
			absolute = true,
			left = (Inline.SLOT - box) / 2,
			top = function(): number
				return over_row:get() * LINE + (LINE - box) / 2
			end,
			Inline.Add {
				on_press = function()
					add_row = over_row:peek()
				end,
				on_release = function()
					ui.after(0, function()
						add_row = nil
						point_at(pointer_y)
					end)
				end,
				on_click = function()
					local win = view.win
					local row = add_row or over_row:peek()
					local r = win and win.rows[row - win.first_row + 1]
					if r then
						add(r.line)
					end
				end,
			},
		})
	end

	-- The text and everything that stands with it, in a scroll view of its
	-- full height. The scroll view is what moves; the window stays put in it
	-- until the view nears its end.
	local function scroll_view(): Node
		return ui.scroll {
			ref = scroller,
			offset = offset,
			-- (Scrolled by hand: a place held from before is let go.)
			on_wheel = function(): boolean?
				holding = nil
				return nil
			end,
			scrollbar = true,
			overscroll = "contain",
			w_full = true,
			h_full = true,
			on_resize = function(size, el)
				-- (A bar opening moves the code under a pointer that stayed put:
				-- which row it is on is known again when it next moves.)
				if size.h ~= room:peek().h then
					point_at(nil)
				end
				room:set({ w = size.w, h = size.h })
				viewport_el = el
			end,
			ui.div {
				w_full = true,
				h = function(): number
					return (placed:get().total + TAIL) * LINE
				end,
				relative = true,
				cursor = "text",
				on_pointer_move = function(e)
					-- (Not while a selection is dragged: nothing to offer then.)
					point_at(if pressing then nil else e.local_y - offset:peek())
				end,
				on_hover = function(inside: boolean)
					if not inside then
						point_at(nil)
					end
				end,
				theme_watch(),
				gutter(),
				ui.div {
					absolute = true,
					left = gutter_w,
					right = 0,
					top = function(): number
						return placed:get().row * LINE
					end,
					h = function(): number
						return math.max(1, placed:get().count) * LINE
					end,
					-- (Here, not round the gutter and the blocks: their buttons
					-- keep their releases to themselves.)
					on_press = function()
						pressing = true
					end,
					on_release = function()
						ui.after(0, function()
							pressing = false
							drag_y = nil
						end)
					end,
					-- A new language makes a new editor: its highlighter is its own.
					-- (And a new indent, a new Tab.)
					ui.div {
						w_full = true,
						h_full = true,
						-- (Clipped: the textarea, as wide as its widest line,
						-- slides in it by `hx`.)
						overflow = "hidden",
						-- (On the textarea itself this loses the theme it is in.)
						theme = { selection = "{zeron.editor.selection}" },
						ui.presence(function(): string
							return `{language() or ""}|{indent:get()}`
						end, function(key: string): Node
							local lang = string.match(key, "^(.-)|")
							return textarea(if lang == "" then nil else lang)
						end, {
							absolute = true,
							top = 0,
							bottom = 0,
							left = function(): number
								return -hx:get()
							end,
							w = laid_w,
						}),
					},
				},
				ui.each(standing, function(b: Block): Node
					return block_node(b)
				end, {
					key = function(b: Block): string
						return b.key
					end,
				}),
				add_control(),
			},
		}
	end

	-- What the find bar says of its matches: "3 of 12", "12" while none is
	-- current, "No results", or what is wrong with a pattern.
	local function tally(): string
		local why = problem:get()
		if why then
			return why
		end
		if query:get() == "" then
			return ""
		end
		local c = count:get()
		if c.n == 0 then
			return locale.s("ds.code_editor.no_results")
		end
		local total = if c.more then `{c.n}+` else tostring(c.n)
		local cur = current:get()
		return if cur > 0 then locale.s("ds.code_editor.count_of", { cur = tostring(cur), total = total }) else total
	end
	local function none(): boolean
		return count:get().n == 0
	end

	-- The bars stand in the pane, above the text, a hairline under them, so
	-- they push the code down and never cover a match: the find bar is a row
	-- of the pane as the viewer's header is. Short of room it folds: the field,
	-- with its count in it, and the arrows stay; the toggles go behind the
	-- chevron with the replace row (and stay out while one is on, so a search
	-- never narrows unseen).
	local function folded_bar(): boolean
		local w = room:get().w
		return w > 0 and w < FIND_FOLD
	end
	local function toggles(): Node
		return ui.row {
			shrink = 0,
			gap = "$zeron.space.hair",
			items = "center",
			toggle("ds.code_editor.match_case", "Aa", case),
			toggle("ds.code_editor.whole_word", "ab", word),
			toggle("ds.code_editor.use_pattern", ".*", regex),
		}
	end
	-- Room for a chevron's width, so a row under the find row lines up with
	-- its field.
	local function under_chevron(): Node
		return ui.div { w = "$zeron.icon_button.sm", shrink = 0 }
	end
	local function bar_row(props_: ContainerProps): ContainerProps
		props_.w_full = true
		props_.gap = "$space.1"
		props_.items = "center"
		return props_
	end
	local find_bar = ui.column {
		w_full = true,
		shrink = 0,
		p = "$space.1",
		gap = "$space.1",
		border_sides = { 0, 0, 1, 0 },
		border_color = "$border.subtle",
		ui.row(bar_row {
			-- The chevron, wherever there is something behind it.
			ui.show(
				function(): boolean
					return not readonly or folded_bar()
				end,
				glyph_button(if readonly then "ds.code_editor.show_options" else "ds.code_editor.toggle_replace", function(): Icon.Name
					return if replacing:get() then "alt-arrow-down" else "alt-arrow-right"
				end, function()
					replacing:toggle()
				end),
				ui.div { w = "$zeron.icon_button.sm", shrink = 0, items = "center", justify = "center", color = "$text.tertiary", Icon { name = "search", size = 14 } }
			),
			TextField {
				input_ref = find_input,
				value = query,
				name = locale.t("ds.code_editor.find") :: any,
				placeholder = function(): string
					return locale.s("ds.code_editor.find")
				end,
				size = "sm",
				grow = 1,
				max_w = FIND_FIELD,
				select_on_focus = true,
				-- What it found, at the field's end, fading if a pattern's
				-- problem runs long.
				trailing = ui.row {
					pr = "$space.1",
					FadeText {
						tally,
						literal = true,
						max_w = FIND_TALLY,
						tabular = true,
						text_size = "$font.size.xs",
						color = function()
							return if problem:get() then "$danger" else "$text.tertiary"
						end,
					},
				},
				on_key = function(e: KeyEvent): boolean?
					local k = string.lower(e.key)
					if k == "enter" then
						step(if e.shift then -1 else 1)
						return true
					end
					return nil
				end,
			},
			ui.show(function(): boolean
				return not folded_bar()
			end, toggles()),
			-- (Whatever the field leaves goes here: it fills to its width first.)
			ui.div { grow = 0.001 },
			ui.row {
				shrink = 0,
				gap = "$zeron.space.hair",
				items = "center",
				glyph_button("ds.code_editor.previous_match", "arrow-up", function()
					step(-1)
				end, none),
				glyph_button("ds.code_editor.next_match", "arrow-down", function()
					step(1)
				end, none),
				glyph_button("ds.code_editor.close", "close", close_bars),
			},
		}),
		ui.show(function(): boolean
			return folded_bar() and (replacing:get() or case:get() or word:get() or regex:get())
		end, ui.row(bar_row { under_chevron(), toggles() })),
		ui.show(
			function()
				return replacing:get() and not readonly
			end,
			ui.row(bar_row {
				under_chevron(),
				TextField {
					value = replacement,
					name = locale.t("ds.code_editor.replace") :: any,
					placeholder = function(): string
						return locale.s("ds.code_editor.replace")
					end,
					size = "sm",
					grow = 1,
					max_w = FIND_FIELD,
					on_key = function(e: KeyEvent): boolean?
						if string.lower(e.key) == "enter" then
							if e.cmd then
								replace_all()
							else
								replace_one()
							end
							return true
						end
						return nil
					end,
				},
				Button { locale.t("ds.code_editor.replace"), tone = "secondary", size = "sm", disabled = none, on_click = replace_one },
				Button { locale.t("ds.code_editor.replace_all"), tone = "secondary", size = "sm", disabled = none, on_click = replace_all },
			})
		),
	}

	local goto_bar = ui.row(bar_row {
		shrink = 0,
		p = "$space.1",
		border_sides = { 0, 0, 1, 0 },
		border_color = "$border.subtle",
		ui.text { locale.t("ds.code_editor.go_to_line_label"), ml = "$space.1", text_size = "$font.size.sm", color = "$text.secondary", shrink = 0, px = "$space.1" },
		TextField {
			input_ref = goto_input,
			value = goto_text,
			name = locale.t("ds.code_editor.line") :: any,
			placeholder = function()
				revision:get()
				return locale.s("ds.code_editor.line_range", { n = tostring(view.n) })
			end,
			size = "sm",
			grow = 1,
			max_w = FIND_FIELD,
			trailing = ui.row { pr = "$space.1", Kbd { keys = "enter", tone = "quiet" } },
			on_key = function(e: KeyEvent): boolean?
				if string.lower(e.key) == "enter" then
					commit_goto()
					return true
				end
				return nil
			end,
		},
	})

	-- An item in the status bar: quiet text, a control when it does
	-- something.
	local function status_item(text: () -> string, run: (() -> ())?, label: Label?): Node
		local t = ui.text { text, literal = true, text_size = "$font.size.xs", tabular = true }
		if not run then
			return ui.row { px = "$space.1", items = "center", color = "$text.tertiary", t }
		end
		return ui.button {
			label = label,
			h = 20,
			px = "$space.1",
			py = 0,
			rounded = "$radius.sm",
			bg = "#00000000",
			hover = { bg = "$hover", color = "$text.secondary" },
			active = { bg = "$pressed", color = "$text.secondary" },
			color = "$text.tertiary",
			focus = { border = 0 },
			on_click = run,
			t,
		}
	end

	local status_bar = ui.row {
		h = "$zeron.editor.status",
		shrink = 0,
		px = "$space.2",
		gap = "$zeron.space.hair",
		items = "center",
		border_sides = { 1, 0, 0, 0 },
		border_color = "$border.subtle",
		status_item(function()
			local c = caret:get()
			local n = math.abs(c.focus - c.anchor)
			local at = { line = tostring(c.line), column = tostring(c.column), n = tostring(n) }
			return if n > 0 then locale.s("ds.code_editor.caret_selected", at) else locale.s("ds.code_editor.caret", at)
		end, open_goto, locale.t("ds.code_editor.go_to_line")),
		ui.div { grow = 1 },
		ui.show(dirty, ui.row {
			gap = "$space.1",
			items = "center",
			px = "$space.1",
			ui.div { w = 6, h = 6, rounded = "$radius.full", bg = "$text.tertiary" },
			ui.text { locale.t("ds.code_editor.edited"), text_size = "$font.size.xs", color = "$text.tertiary" },
		}),
		status_item(function()
			local u = indent:get()
			return if u == "\t" then locale.s("ds.code_editor.tabs") else locale.s("ds.code_editor.spaces", { n = tostring(#u) })
		end),
		status_item(function()
			return if wrap:get() then locale.s("ds.code_editor.wrap") else locale.s("ds.code_editor.no_wrap")
		end, function()
			wrap:set(not wrap:peek())
		end, locale.t("ds.code_editor.toggle_soft_wrap")),
		status_item(function()
			return Syntax.name(language())
		end),
	}

	-- The right-click menu: the clipboard, then finding, the lines and folds.
	local function menu(): { ContextMenu.Entry }?
		local a, f = selection()
		local has = a ~= f
		local function copy()
			local lo, hi = math.min(a, f), math.max(a, f)
			ui.clipboard.write(string.sub(view.text, lo + 1, hi))
		end
		local entries: { ContextMenu.Entry } = {}
		if not readonly then
			table.insert(entries, {
				label = locale.s("ds.code_editor.cut"),
				shortcut = "cmd+x",
				disabled = not has,
				on_select = function()
					copy()
					apply(math.min(a, f), math.max(a, f), "", math.min(a, f))
				end,
			})
		end
		table.insert(entries, { label = locale.s("common.copy"), icon = "copy" :: Icon.Name, shortcut = "cmd+c", disabled = not has, on_select = copy })
		if not readonly then
			table.insert(entries, {
				label = locale.s("ds.code_editor.paste"),
				shortcut = "cmd+v",
				on_select = function()
					local clip = ui.clipboard.read()
					if clip then
						local lo, hi = math.min(a, f), math.max(a, f)
						apply(lo, hi, clip, lo + #clip)
					end
				end,
			})
		end
		table.insert(entries, {
			label = locale.s("ds.code_editor.select_all"),
			shortcut = "cmd+a",
			on_select = function()
				place(0, #view.text)
			end,
		})
		if props.on_add_block then
			local add = props.on_add_block
			table.insert(entries, { separator = true })
			table.insert(entries, {
				label = locale.s("ds.code_editor.add_comment"),
				on_select = function()
					add(view:line_of(f))
				end,
			})
		end
		table.insert(entries, { separator = true })
		table.insert(entries, {
			label = locale.s("ds.code_editor.find"),
			icon = "search" :: Icon.Name,
			shortcut = "cmd+f",
			on_select = function()
				open_find(false)
			end,
		})
		table.insert(entries, { label = locale.s("ds.code_editor.go_to_line_menu"), shortcut = "ctrl+g", on_select = open_goto })
		if not readonly then
			table.insert(entries, { separator = true })
			table.insert(entries, {
				label = locale.s("ds.code_editor.toggle_comment"),
				shortcut = "cmd+/",
				on_select = function()
					toggle_comment()
				end,
			})
			table.insert(entries, {
				label = locale.s("ds.code_editor.move_line_up"),
				shortcut = "alt+up",
				on_select = function()
					move(false)
				end,
			})
			table.insert(entries, {
				label = locale.s("ds.code_editor.move_line_down"),
				shortcut = "alt+down",
				on_select = function()
					move(true)
				end,
			})
			table.insert(entries, {
				label = locale.s("ds.code_editor.duplicate_line"),
				shortcut = "shift+alt+down",
				on_select = function()
					duplicate(true)
				end,
			})
		end
		if folding and next(view.by_first) ~= nil then
			table.insert(entries, { separator = true })
			table.insert(entries, {
				label = locale.s("ds.code_editor.fold"),
				shortcut = "alt+cmd+[",
				on_select = function()
					fold_here(false)
				end,
			})
			table.insert(entries, {
				label = locale.s("ds.code_editor.unfold"),
				shortcut = "alt+cmd+]",
				on_select = function()
					fold_here(true)
				end,
			})
			table.insert(entries, { label = locale.s("ds.code_editor.fold_all"), shortcut = "shift+alt+cmd+[", on_select = fold_all })
			table.insert(entries, { label = locale.s("ds.code_editor.unfold_all"), shortcut = "shift+alt+cmd+]", on_select = unfold_all })
		end
		table.insert(entries, { separator = true })
		table.insert(entries, {
			label = locale.s("ds.code_editor.soft_wrap"),
			shortcut = "alt+z",
			checked = function()
				return wrap:get()
			end,
			on_select = function()
				wrap:set(not wrap:peek())
			end,
		})
		return entries
	end

	local frame: { [any]: any } = {
		on_key = on_frame_key,
		overflow = "hidden",
	}
	for k, v in props :: { [any]: any } do
		if not OWN[k] then
			frame[k] = v
		end
	end
	table.insert(
		frame,
		ui.column {
			w_full = true,
			h_full = true,
			ui.show(find_open, find_bar),
			ui.show(goto_open, goto_bar),
			ContextMenu {
				items = menu,
				grow = 1,
				w_full = true,
				h = 0,
				scroll_view(),
			},
			if show_status then status_bar else nil,
		}
	)
	return ui.overlay(frame)
end)
