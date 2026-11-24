-- Markdown as it is typed, as Zeron's composer edits it. The draft stays
-- plain markdown, every marker in it as typed; this only says how to show
-- it and how a line break continues it.
--
-- `highlight(line, state)` is a textarea's `highlight`: per line, spans of
-- `{ from, to, color, weight?, italic?, strike?, bg?, size?, line_height?,
-- font? }` (bytes from 1, `to` the last). The markdown stays in view,
-- greyed: emphasis markers, backticks, a heading's `#`s, a quote's `>`,
-- fences and rules. Bold, italic, struck and code text take their face; a
-- heading its size, code the mono face.
--
-- `line_insets(line, state)` and `line_marks(line, state)` are the
-- textarea's too: the room a line takes and the shapes beside it. A fenced
-- block is a washed block whose lines never wrap (they scroll sideways
-- together); a quote is set in from a rail; a rule is drawn across; a list
-- item hangs from its marker, its wrapped lines under its text. (Its
-- marker is drawn beside the item, the very marker typed: the characters
-- themselves are kept in the text, folded to nothing, so the item's text
-- starts where its wrapped lines do. `marker_caret` keeps the caret out of
-- them.)
--
-- The state is what the lines above leave: inside a fenced block, the
-- block's number (from 1, so each block scrolls on its own); outside one,
-- 0 less the blocks closed above it. `inside(state)` says which.
--
-- What it's drawn in (`Look`) is the theme's (zeron.composer and
-- zeron.composer.markdown), read once by `look()`: the textarea's
-- callbacks must depend on nothing but their arguments, so `make(look)`
-- gives callbacks holding it. `M.highlight` and its kin hold the theme's
-- look as it was when first asked (before the theme's set up, no sizes:
-- the colours and faces alone).
--
-- `newline(text, a, b)` is what Shift+Enter does in a list: the next item's
-- marker after the break, or, on an empty item, the item undone (outdented
-- a level). Bytes from 0, as an input's selection gives them.

local M = {}

local MARKER = "$text.tertiary"
local CODE_WASH = "$zeron.code.wash"
local QUOTE_INK = "$zeron.composer.markdown.quote.ink"
local QUOTE_RAIL = "$zeron.composer.markdown.quote.rail"
local RULE = "$zeron.composer.markdown.rule.ink"
-- (A list's marker, folded: drawn beside the item instead.)
local GONE = "#00000000"
-- The least a span's size can be (zgx's): what a folded marker is set in.
local FOLDED = 1

-- What the draft's markdown is drawn in, in points. Any of it missing (no
-- theme yet), that part is drawn as plain text is.
export type Look = {
	-- The draft's own type: its size and line pitch.
	text: number?,
	line: number?,
	-- Headings' size and pitch, by level (1 to 3; deeper ones are bold
	-- body text).
	heading: { { size: number?, line: number? } },
	-- Code (inline and in a block): the mono face's size; a block's text's
	-- inset from its wash.
	code: number?,
	code_inset: number?,
	-- A quote's rail (its width) and its text's inset.
	quote_rail: number?,
	quote_inset: number?,
	-- A rule's thickness.
	rule: number?,
	-- A list: the step a level of nesting sets it in, the least room its
	-- marker takes, and the gap after the marker.
	list_indent: number?,
	list_marker: number?,
	list_gap: number?,
}

-- The look the theme gives, read now.
function M.look(): Look
	local function num(name: string): number?
		local ok, v = pcall(function()
			return ui.theme.get(name)
		end)
		return if ok and type(v) == "number" then v else nil
	end
	local md = "zeron.composer.markdown"
	return {
		text = num("zeron.composer.text"),
		line = num("zeron.composer.line"),
		heading = {
			{ size = num(`{md}.h1.size`), line = num(`{md}.h1.line`) },
			{ size = num(`{md}.h2.size`), line = num(`{md}.h2.line`) },
			{ size = num(`{md}.h3.size`), line = num(`{md}.h3.line`) },
		},
		code = num(`{md}.code.size`),
		code_inset = num(`{md}.code.inset`),
		quote_rail = num(`{md}.quote.rail_w`),
		quote_inset = num(`{md}.quote.inset`),
		rule = num(`{md}.rule.h`),
		list_indent = num(`{md}.list.indent`),
		list_marker = num(`{md}.list.marker`),
		list_gap = num(`{md}.list.gap`),
	}
end

type Props = { [string]: any }
type Mark = { a: number, b: number, p: Props }

-- What a list line starts with: its indent, its marker, the space after it
-- (and a task box), where the item's text starts, and the next item's
-- marker. `fold`: how much of it is the marker drawn beside the item (its
-- indent, the marker, the spaces after it; a task box stays in the text).
type Prefix = { indent: string, marker: string, stop: number, fold: number, next: string, number: boolean }
local function prefix(line: string): Prefix?
	local indent, bullet, space = string.match(line, "^(%s*)([-*+])(%s+)")
	if indent and bullet and space then
		local stop = #indent + #bullet + #space
		local fold = stop
		local box = string.match(line, "^%[[ xX]%]%s+", stop + 1)
		if box then
			stop += #box
		end
		return {
			indent = indent,
			marker = bullet,
			stop = stop,
			fold = fold,
			next = bullet .. space .. (if box then "[ ] " else ""),
			number = false,
		}
	end
	local digits: string?, close: string?
	indent, digits, close, space = string.match(line, "^(%s*)(%d+)([.)])(%s+)")
	if indent and digits and close and space then
		local stop = #indent + #digits + #close + #space
		return {
			indent = indent,
			marker = digits .. close,
			stop = stop,
			fold = stop,
			next = tostring((tonumber(digits) :: number) + 1) .. close .. space,
			number = true,
		}
	end
	return nil
end

-- A rule: three or more of one of - * _, spaces between.
local function is_rule(line: string): boolean
	local c = string.match(line, "^%s*([-*_])")
	if not c then
		return false
	end
	local count = 0
	for ch in string.gmatch(line, "%S") do
		if ch ~= c then
			return false
		end
		count += 1
	end
	return count >= 3
end

local function is_fence(line: string): boolean
	return string.match(line, "^%s*```") ~= nil or string.match(line, "^%s*~~~") ~= nil
end

-- Whether a line given `state` is inside a fenced block.
function M.inside(state: any): boolean
	return type(state) == "number" and state > 0
end

-- What a line is, given the state the line above left: a fenced block's
-- fence or line (and the block's number), a heading (its level), a rule,
-- a quote, a list item, or prose.
type Kind = {
	kind: "fence" | "code" | "heading" | "rule" | "quote" | "item" | "text",
	block: number?,
	level: number?,
	item: Prefix?,
}
local function kind_of(line: string, state: any): Kind
	local fence = is_fence(line)
	if M.inside(state) then
		return { kind = if fence then "fence" else "code", block = state }
	end
	if fence then
		-- (Opening: the block after those closed above.)
		return { kind = "fence", block = 1 - (if type(state) == "number" then state else 0) }
	end
	if #line > 0 and is_rule(line) then
		return { kind = "rule" }
	end
	local hashes = string.match(line, "^%s*(#+)%s+")
	if hashes and #hashes <= 6 then
		return { kind = "heading", level = #hashes }
	end
	if string.match(line, "^%s*>") then
		return { kind = "quote" }
	end
	local p = prefix(line)
	if p then
		return { kind = "item", item = p }
	end
	return { kind = "text" }
end

-- Inline faces from `from` on: code spans first (nothing inside them is
-- markdown), then ~~strike~~, **bold** / __bold__, *italic* / _italic_.
-- Each found pair's markers are masked out of a copy of the line so the
-- next pass doesn't take them again.
local function inline(look: Look, line: string, from: number, marks: { Mark })
	local function mark(a: number, b: number, p: Props)
		if b >= a then
			table.insert(marks, { a = a, b = b, p = p })
		end
	end
	local masked = line
	local function mask(a: number, b: number)
		masked = string.sub(masked, 1, a - 1) .. string.rep("\1", b - a + 1) .. string.sub(masked, b + 1)
	end
	-- Code: a run of backticks closed by the same run, in the mono face
	-- (its backticks too, so the run keeps its width).
	local mono: Props = if look.code then { font = "mono", size = look.code } else {}
	local function with(p: Props, more: Props): Props
		local out = table.clone(p)
		for k, v in more do
			out[k] = v
		end
		return out
	end
	local i = from
	while true do
		local s, e = string.find(line, "`+", i)
		if not s or not e then
			break
		end
		local ticks = string.sub(line, s, e)
		local cs, ce = string.find(line, ticks, e + 1, true)
		if not cs or ce == nil then
			break
		end
		-- The very run (not a longer one).
		if string.sub(line, ce + 1, ce + 1) == "`" then
			i = ce + 1
			continue
		end
		mark(s, e, with(mono, { color = MARKER }))
		mark(e + 1, cs - 1, with(mono, { bg = CODE_WASH }))
		mark(cs, ce, with(mono, { color = MARKER }))
		mask(s, ce)
		i = ce + 1
	end
	-- Pairs: an opener, text starting with no space, the closer.
	local function pairs_of(open: string, pattern: string, face: Props, intraword: boolean)
		local at = from
		while true do
			local s, e = string.find(masked, pattern, at)
			if not s or e == nil then
				break
			end
			local n = #open
			-- `_` inside a word (snake_case) is no emphasis.
			local before = if s > 1 then string.sub(masked, s - 1, s - 1) else ""
			local after = string.sub(masked, e + 1, e + 1)
			if
				not intraword
				and (string.match(before, "[%w]") ~= nil or string.match(after, "[%w]") ~= nil)
			then
				at = s + 1
				continue
			end
			-- No closer right after a space ("a * b *").
			if string.sub(masked, e - n, e - n) == " " then
				at = s + 1
				continue
			end
			mark(s, s + n - 1, { color = MARKER })
			mark(s + n, e - n, face)
			mark(e - n + 1, e, { color = MARKER })
			mask(s, s + n - 1)
			mask(e - n + 1, e)
			at = e + 1
		end
	end
	pairs_of("~~", "~~[^%s~\1].-~~", { strike = true }, true)
	pairs_of("**", "%*%*[^%s%*\1].-%*%*", { weight = 600 }, true)
	pairs_of("__", "__[^%s_\1].-__", { weight = 600 }, false)
	pairs_of("*", "%*[^%s%*\1][^%*]-%*", { italic = true }, true)
	pairs_of("_", "_[^%s_\1][^_]-_", { italic = true }, false)
end

-- A span's faces, by name.
local FACES = { "weight", "italic", "strike", "bg", "size", "line_height", "font" }

-- The marks, overlapping as faces nest, cut into runs each with all the
-- faces over it (later marks win a field).
local function runs(marks: { Mark }): { { any } }
	if #marks == 0 then
		return {}
	end
	local cuts: { number } = {}
	for _, m in marks do
		table.insert(cuts, m.a)
		table.insert(cuts, m.b + 1)
	end
	table.sort(cuts)
	local out: { { any } } = {}
	local last: number? = nil
	for _, c in cuts do
		if last and c > last then
			local p: Props = {}
			local any = false
			for _, m in marks do
				if m.a <= last and m.b >= c - 1 then
					for k, v in m.p do
						p[k] = v
					end
					any = true
				end
			end
			if any then
				local span: { any } = { last, c - 1, p.color or "$text.primary" }
				local s = span :: any
				for _, face in FACES do
					s[face] = p[face]
				end
				table.insert(out, span)
			end
		end
		last = c
	end
	return out
end

-- The mono face for a whole line of code (`look`'s, at the draft's pitch).
local function code_face(look: Look): Props
	return if look.code then { font = "mono", size = look.code, line_height = look.line } else {}
end

local function highlight(look: Look, line: string, state: any): ({ { any } }, any)
	local n = #line
	local k = kind_of(line, state)
	if k.kind == "fence" then
		local next = if M.inside(state) then -(state :: number) else k.block
		if n == 0 then
			return {}, next
		end
		local p = code_face(look)
		p.color = MARKER
		return runs({ { a = 1, b = n, p = p } }), next
	end
	if k.kind == "code" then
		if n == 0 then
			return {}, state
		end
		return runs({ { a = 1, b = n, p = code_face(look) } }), state
	end
	local out = if type(state) == "number" then state else 0
	if n == 0 then
		return {}, out
	end
	if k.kind == "rule" then
		return { { 1, n, MARKER } }, out
	end
	local marks: { Mark } = {}
	local from = 1
	if k.kind == "heading" then
		-- A heading: its #s greyed, its text bold, the whole line at its
		-- level's size.
		local indent, hashes, space = string.match(line, "^(%s*)(#+)(%s+)")
		local stop = #(indent :: string) + #(hashes :: string) + #(space :: string)
		local step = look.heading[k.level :: number]
		if step and step.size then
			table.insert(marks, { a = 1, b = n, p = { size = step.size, line_height = step.line } })
		end
		table.insert(marks, { a = #(indent :: string) + 1, b = stop, p = { color = MARKER } })
		table.insert(marks, { a = stop + 1, b = n, p = { weight = 600 } })
		from = stop + 1
	elseif k.kind == "quote" then
		-- A quote's > in the marker's ink, its words in the quote's (a list
		-- in it keeps its marker in the text, greyed).
		local q, gt = string.match(line, "^(%s*)(>%s?)")
		table.insert(marks, { a = 1, b = n, p = { color = QUOTE_INK } })
		table.insert(marks, { a = #(q :: string) + 1, b = #(q :: string) + #(gt :: string), p = { color = MARKER } })
		from = #(q :: string) + #(gt :: string) + 1
		local p = prefix(string.sub(line, from))
		if p then
			local at = from - 1 + #p.indent
			table.insert(marks, { a = at + 1, b = at + #p.marker, p = { color = MARKER } })
			from = from + p.stop
		end
	elseif k.kind == "item" then
		-- A list item: its marker (with its indent and the spaces after it)
		-- folded, drawn beside the item instead (`line_marks`); a task box
		-- greyed.
		local p = k.item :: Prefix
		table.insert(marks, { a = 1, b = p.fold, p = { color = GONE, size = FOLDED, line_height = look.line } })
		if p.stop > p.fold then
			table.insert(marks, { a = p.fold + 1, b = p.stop, p = { color = MARKER } })
		end
		from = p.stop + 1
	end
	inline(look, line, from, marks)
	return runs(marks), out
end

-- How wide a list's marker is drawn, by the marker and its size: measured
-- once each.
local widths: { [string]: number } = {}
local function marker_width(marker: string, size: number, tabular: boolean): number
	local key = `{marker}\0{size}\0{tabular}`
	local w = widths[key]
	if w == nil then
		-- (Measured where there's an interface to measure in: the words'
		-- tests have none, and lay it out at its least.)
		local ok, m = pcall(function()
			return ui.measure_text(marker, { size = size, tabular = tabular })
		end)
		w = if ok and type(m) == "table" and type(m.w) == "number" then m.w else 0
		widths[key] = w
	end
	return w :: number
end

-- A list item's nesting (by its indent: two spaces a level, a tab two),
-- where its marker is drawn and where its text starts.
local function item_room(look: Look, p: Prefix): (number, number)
	local spaces = 0
	for ch in string.gmatch(p.indent, ".") do
		spaces += if ch == "\t" then 4 else 1
	end
	local x = math.floor(spaces / 2) * (look.list_indent or 0)
	local least = look.list_marker or 0
	local w = if look.text then marker_width(p.marker, look.text, p.number) else 0
	return x, x + math.max(least, w + (look.list_gap or 0))
end

local function line_insets(look: Look, line: string, state: any): { [string]: any }?
	local k = kind_of(line, state)
	if k.kind == "code" then
		return { left = look.code_inset, group = k.block, nowrap = true }
	elseif k.kind == "fence" then
		return if look.code_inset then { left = look.code_inset } else nil
	elseif k.kind == "quote" then
		return if look.quote_inset then { left = look.quote_inset } else nil
	elseif k.kind == "item" then
		local _, left = item_room(look, k.item :: Prefix)
		return if left > 0 then { left = left } else nil
	end
	return nil
end

local function line_marks(look: Look, line: string, state: any): { { [string]: any } }?
	local k = kind_of(line, state)
	if k.kind == "code" or k.kind == "fence" then
		-- The block's wash, edge to edge, still as its lines scroll.
		return { { x = 0, w = "fill", h = "fill", fill = CODE_WASH, pinned = true } }
	elseif k.kind == "quote" then
		return if look.quote_rail then { { x = 0, w = look.quote_rail, h = "fill", fill = QUOTE_RAIL } } else nil
	elseif k.kind == "rule" then
		if not look.rule or not look.line then
			return nil
		end
		-- (Through the middle of the line, where its dashes are.)
		return { { x = 0, y = math.floor(look.line / 2), w = "fill", h = look.rule, fill = RULE } }
	elseif k.kind == "item" then
		local p = k.item :: Prefix
		local x, left = item_room(look, p)
		if left <= 0 or not look.line then
			return nil
		end
		return {
			{
				x = x,
				y = 0,
				w = left - x,
				h = look.line,
				label = p.marker,
				label_color = MARKER,
				label_size = look.text,
				tabular = p.number,
			},
		}
	end
	return nil
end

export type Callbacks = {
	highlight: (line: string, state: any) -> ({ { any } }, any),
	line_insets: (line: string, state: any) -> { [string]: any }?,
	line_marks: (line: string, state: any) -> { { [string]: any } }?,
}

-- The textarea's callbacks, drawn in `look`.
function M.make(look: Look): Callbacks
	return {
		highlight = function(line: string, state: any): ({ { any } }, any)
			return highlight(look, line, state)
		end,
		line_insets = function(line: string, state: any): { [string]: any }?
			return line_insets(look, line, state)
		end,
		line_marks = function(line: string, state: any): { { [string]: any } }?
			return line_marks(look, line, state)
		end,
	}
end

-- The theme's, as first asked for.
local default: Callbacks? = nil
local function defaults(): Callbacks
	if default == nil then
		default = M.make(M.look())
	end
	return default :: Callbacks
end
function M.highlight(line: string, state: any): ({ { any } }, any)
	return defaults().highlight(line, state)
end
function M.line_insets(line: string, state: any): { [string]: any }?
	return defaults().line_insets(line, state)
end
function M.line_marks(line: string, state: any): { { [string]: any } }?
	return defaults().line_marks(line, state)
end

-- Where the caret goes when it lands in a list item's folded marker (it
-- shows as one place, the item's text's start): `line` the caret's line
-- (not inside a fenced block), `at` the caret in it and `was` where it was
-- (bytes from 0 in the line, `was` nil if it was on another line). The
-- item's text's start, or, stepped back into the marker from there, -1:
-- the end of the line before. Nil where it may stay.
function M.marker_caret(line: string, at: number, was: number?): number?
	local p = prefix(line)
	if not p or is_rule(line) or at >= p.fold then
		return nil
	end
	if was == p.fold and at == p.fold - 1 then
		return -1
	end
	return p.fold
end

-- How many of the lines in the first `upto` bytes of `text` start with
-- `fence` after blanks (`%s`): found with a plain search, which skips the
-- lines between them rather than matching each.
local function fences(text: string, upto: number, fence: string): number
	local count, from = 0, 1
	while true do
		local s = string.find(text, fence, from, true)
		if s == nil or s + #fence - 1 > upto then
			return count
		end
		local i = s - 1
		while i >= 1 do
			local c = string.byte(text, i)
			if c == 10 or (c ~= 32 and (c < 9 or c > 13)) then
				break
			end
			i -= 1
		end
		if i < 1 or string.byte(text, i) == 10 then
			count += 1
		end
		local nl = string.find(text, "\n", s + #fence, true)
		if nl == nil then
			return count
		end
		from = nl + 1
	end
end

-- What a line break does at the selection `a`..`b` (bytes from 0) in a
-- list item: `(from, to, text)` to put in place of `from`..`to`, or nil
-- for a plain break.
function M.newline(text: string, a: number, b: number): (number?, number?, string?)
	if a ~= b then
		return nil, nil, nil
	end
	-- The caret's line, in bytes from 0: its start looked for back from the
	-- caret a window at a time, not from the draft's top (a break in a
	-- draft of 30,000 lines walked them all, then matched them all for
	-- fences, 20 ms of an Enter).
	local start = 0
	local hi = a
	while hi > 0 do
		local lo = math.max(1, hi - 1023)
		local after = string.match(string.sub(text, lo, hi), "^.*\n()")
		if after then
			start = lo + after - 2
			break
		end
		hi = lo - 1
	end
	local stop = string.find(text, "\n", a + 1, true)
	local line_end = if stop then stop - 1 else #text
	local line = string.sub(text, start + 1, line_end)
	local p = prefix(line)
	if not p or a < start + p.stop then
		return nil, nil, nil
	end
	-- Not inside a fenced block: count the fences above (lines
	-- starting, after blanks, with ``` or ~~~: never both).
	if (fences(text, start, "```") + fences(text, start, "~~~")) % 2 == 1 then
		return nil, nil, nil
	end
	if string.match(string.sub(line, p.stop + 1), "^%s*$") then
		-- An empty item: undone, a level out.
		return start, line_end, string.sub(p.indent, 1, math.max(0, #p.indent - 2))
	end
	return a, a, "\n" .. p.indent .. p.next
end

return M
