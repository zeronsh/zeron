-- What a code editor shows of its file. zgx's textarea has no way to hide a
-- line or leave room between two, and its scrolling is its own; so the editor
-- gives it text to show rather than the file: a window of a few screens of
-- lines, the folded ones left out, a run of empty rows where a block stands
-- between two lines, and a long line broken into rows if it wraps. Those are
-- all this module's. It knows where each row of the window comes from, so a
-- caret, a selection or an edit in the window is a place in the file, and
-- back.
--
-- Three numbers place everything: a file offset is in bytes from 0, as the
-- textarea's selection counts; a line is from 1; a row is a row of what
-- shows, from 0, a fixed height each. `top[line]` is the rows above a line
-- (a hidden line has none of its own, so it has the same `top` as the next),
-- one number a line: made again when lines or folds change, a few hundred
-- nanoseconds a line, and read by a binary search everywhere else.
--
-- Nothing here touches the interface: `ds/code_editor` drives it.

local M = {}
M.__index = M

-- The folded line's summary, drawn after its text: ` ⋯ }  12 lines`.
M.ELLIPSIS = " ⋯ "

export type Region = { first: number, last: number, closer: string? }
export type Row = {
	line: number,
	-- The file's bytes it shows: `from` to `to`, then `extra` bytes that
	-- aren't the file's (a fold's summary).
	from: number,
	to: number,
	extra: number,
	-- Empty, under a block.
	spacer: boolean?,
	-- A wrapped line's second row or later.
	more: boolean?,
}
export type Window = {
	first_row: number,
	rows: { Row },
	-- Where each row starts in `text`.
	at: { number },
	text: string,
	-- The first and last lines in it.
	first_line: number,
	last_line: number,
	-- What the folded lines read, by their text: the highlighter colours
	-- the summary.
	summaries: { [string]: number },
	-- A wrapped line's rows, by their text: `{ line, from, to, last }`.
	wrapped: { [string]: { any } },
}

type Block = { line: number, rows: number, given: number }
type Fields = {
	text: string,
	starts: { number },
	n: number,
	-- The rows above each line (see above).
	top: { number },
	regions: { Region },
	by_first: { [number]: Region },
	folded: { [number]: Region },
	blocks: { [string]: Block },
	cols: number?,
	tab: number,
	breaks: { [number]: { number } },
	win: Window?,
}
export type CodeView = setmetatable<Fields, typeof(M)>

function M.new(text: string): CodeView
	local self: CodeView = setmetatable({
		text = "",
		starts = {} :: { number },
		n = 0,
		top = {} :: { number },
		regions = {} :: { Region },
		by_first = {} :: { [number]: Region },
		folded = {} :: { [number]: Region },
		-- Blocks standing under lines, by key: their line, their rows, and the
		-- line they were last given or told of.
		blocks = {} :: { [string]: Block },
		-- Wrapping at this many columns (nil: not at all), and where the
		-- wrapped lines break: bytes from the line's start.
		cols = nil :: number?,
		tab = 8,
		breaks = {} :: { [number]: { number } },
		win = nil :: Window?,
	}, M)
	self:reset(text)
	return self
end

---------------------------------------------------------------------------
-- The file.

-- A new text: no folds, no history of the old one.
function M.reset(self: CodeView, text: string)
	self.text = text
	local starts = table.create(64) :: { number }
	starts[1] = 0
	local n, at = 1, 0
	while true do
		local nl = string.find(text, "\n", at + 1, true)
		if not nl then
			break
		end
		n += 1
		starts[n] = nl
		at = nl
	end
	self.starts, self.n = starts, n
	self.regions, self.by_first, self.folded = {}, {}, {}
	self.breaks = {}
	self:layout()
end

function M.line_start(self: CodeView, line: number): number
	return self.starts[line]
end

-- Where a line ends, before its newline.
function M.line_end(self: CodeView, line: number): number
	local nxt = self.starts[line + 1]
	return if nxt then nxt - 1 else #self.text
end

function M.line_text(self: CodeView, line: number): string
	return string.sub(self.text, self.starts[line] + 1, self:line_end(line))
end

-- The line holding offset `p`.
function M.line_of(self: CodeView, p: number): number
	local starts = self.starts
	local lo, hi = 1, self.n
	while lo < hi do
		local mid = (lo + hi + 1) // 2
		if starts[mid] <= p then
			lo = mid
		else
			hi = mid - 1
		end
	end
	return lo
end

-- Bytes `from` to `to` become `ins`: the text and the lines' starts, and
-- what stands on lines moved along. Returns whether the lines changed.
function M.splice(self: CodeView, from: number, to: number, ins: string): boolean
	local text = self.text
	local a, b = self:line_of(from), self:line_of(to)
	self.text = string.sub(text, 1, from) .. ins .. string.sub(text, to + 1)
	local d = #ins - (to - from)
	local added: { number } = {}
	if #ins > 0 then
		local at = 0
		while true do
			local nl = string.find(ins, "\n", at + 1, true)
			if not nl then
				break
			end
			table.insert(added, from + nl)
			at = nl
		end
	end
	local starts = self.starts
	local n = self.n
	local gone, got = b - a, #added
	if got ~= gone then
		-- The lines after, moved to their new places.
		table.move(starts, b + 1, n, a + got + 1)
		if got < gone then
			for i = n - (gone - got) + 1, n do
				starts[i] = nil
			end
		end
	end
	for i = 1, got do
		starts[a + i] = added[i]
	end
	n += got - gone
	self.n = n
	if d ~= 0 then
		for i = a + got + 1, n do
			starts[i] += d
		end
	end
	if got ~= gone then
		self:shift(a, b, got - gone)
	end
	return got ~= gone
end

-- Lines `a` to `b` are now `delta` more: what stands on them keeps its line.
-- A fold whose head went is open again; nothing hides by surprise.
function M.shift(self: CodeView, a: number, b: number, delta: number)
	local function moved(line: number): number
		if line > b then
			return line + delta
		end
		return math.min(line, a + math.max(0, b - a + delta))
	end
	local folded: { [number]: Region } = {}
	for first, r in self.folded do
		if first <= a or first > b then
			local nf = if first > b then first + delta else first
			local nl = moved(r.last)
			if nl > nf then
				folded[nf] = { first = nf, last = nl, closer = r.closer }
			end
		end
	end
	self.folded = folded
	for _, r in self.regions do
		r.first = moved(r.first)
		r.last = moved(r.last)
	end
	self:index_regions()
	for _, block in self.blocks do
		block.line = moved(block.line)
	end
end

---------------------------------------------------------------------------
-- Folds.

function M.index_regions(self: CodeView)
	local by: { [number]: Region } = {}
	for _, r in self.regions do
		if r.last > r.first then
			by[r.first] = r
		end
	end
	self.by_first = by
end

-- The regions found again: what was folded stays so where its head still
-- opens a region, keeping the region's new extent.
function M.set_regions(self: CodeView, regions: { Region }): boolean
	self.regions = regions
	self:index_regions()
	local folded: { [number]: Region } = {}
	local changed = false
	for first, old in self.folded do
		local r = self.by_first[first]
		if r then
			folded[first] = { first = first, last = r.last, closer = r.closer }
			changed = changed or r.last ~= old.last
		else
			-- Its head stopped opening one: it opens, all of it showing.
			changed = true
		end
	end
	self.folded = folded
	return changed
end

function M.is_folded(self: CodeView, line: number): boolean
	return self.folded[line] ~= nil
end

function M.fold(self: CodeView, line: number): boolean
	local r = self.by_first[line]
	if not r or self.folded[line] then
		return false
	end
	self.folded[line] = { first = r.first, last = r.last, closer = r.closer }
	return true
end

function M.unfold(self: CodeView, line: number): boolean
	if not self.folded[line] then
		return false
	end
	self.folded[line] = nil
	return true
end

function M.fold_all(self: CodeView)
	for first in self.by_first do
		self:fold(first)
	end
end

function M.unfold_all(self: CodeView): boolean
	local any = next(self.folded) ~= nil
	self.folded = {}
	return any
end

-- The region to act on for a caret on `line`: the innermost that holds it.
function M.region_at(self: CodeView, line: number): Region?
	local best: Region? = nil
	if self.by_first[line] then
		return self.by_first[line]
	end
	for _, r in self.regions do
		if r.first > line then
			break
		end
		if r.last >= line and (not best or r.first > best.first) then
			best = r
		end
	end
	return best
end

-- Every fold hiding `line` (or lines `line` to `to`) opened: true if any.
function M.open_around(self: CodeView, line: number, to: number?): boolean
	local last = to or line
	local any = false
	for first, r in self.folded do
		if first < last and r.last >= line then
			self.folded[first] = nil
			any = true
		end
	end
	return any
end

---------------------------------------------------------------------------
-- Rows.

-- Where a long line breaks into rows of `cols`: after a space where one is
-- in reach, else mid-word, never inside a character. A tab takes `tab`
-- columns and a character of three bytes or more two, near enough what they
-- draw: a row a little long scrolls; it never misplaces the row under it.
local function breaks_of(s: string, cols: number, tab: number): { number }?
	local n = #s
	if n <= cols and not string.find(s, "\t", 1, true) then
		return nil
	end
	local out: { number }? = nil
	local col, i = 0, 1
	local start = 1
	local space = 0
	while i <= n do
		local b = string.byte(s, i)
		local w, len = 1, 1
		if b == 9 then
			w = tab
		elseif b >= 0xF0 then
			w, len = 2, 4
		elseif b >= 0xE0 then
			w, len = 2, 3
		elseif b >= 0xC0 then
			len = 2
		end
		if col + w > cols and i > start then
			local cut = if space > start then space else i
			if not out then
				out = {}
			end
			table.insert(out :: { number }, cut - 1)
			start = cut
			col = 0
			space = 0
			i = cut
		else
			col += w
			i += len
			-- (After a space, or punctuation code breaks well after.)
			if b == 32 or b == 44 or b == 40 or b == 91 or b == 123 or b == 47 then
				space = i
			end
		end
	end
	return out
end

-- The rows again: after lines, folds, blocks or the wrapping width change.
function M.layout(self: CodeView)
	local top = self.top
	local n = self.n
	local cols = self.cols
	local folded = self.folded
	local starts = self.starts
	local spacers: { [number]: number }? = nil
	for _, block in self.blocks do
		spacers = spacers or {}
		local s = spacers :: { [number]: number }
		s[block.line] = (s[block.line] or 0) + block.rows
	end
	local breaks: { [number]: { number } } = {}
	local text = self.text
	local r = 0
	local hide_to = 0
	local tab = self.tab
	for i = 1, n do
		top[i] = r
		if i > hide_to then
			r += 1
			if cols then
				local a = starts[i]
				local e = (starts[i + 1] or (#text + 1)) - 1
				if e - a > cols or (e > a and string.byte(text, a + 1) == 9) then
					local b = breaks_of(string.sub(text, a + 1, e), cols, tab)
					if b then
						breaks[i] = b
						r += #b
					end
				end
			end
			if spacers and spacers[i] then
				r += spacers[i]
			end
			local f = folded[i]
			if f then
				hide_to = f.last
			end
		end
	end
	top[n + 1] = r
	for i = #top, n + 2, -1 do
		top[i] = nil
	end
	self.breaks = breaks
end

function M.total_rows(self: CodeView): number
	return self.top[self.n + 1]
end

function M.is_hidden(self: CodeView, line: number): boolean
	return self.top[line] == self.top[line + 1]
end

-- The first row of a line (hidden: the row of the line after its fold).
function M.row_of(self: CodeView, line: number): number
	return self.top[line]
end

-- The line a row belongs to.
function M.line_at(self: CodeView, row: number): number
	local top = self.top
	local lo, hi = 1, self.n
	while lo < hi do
		local mid = (lo + hi + 1) // 2
		if top[mid] <= row then
			lo = mid
		else
			hi = mid - 1
		end
	end
	return lo
end

-- The line showing for `line`: itself, or the head of the fold hiding it.
function M.shown_line(self: CodeView, line: number): number
	if not self:is_hidden(line) then
		return line
	end
	local top = self.top
	local t = top[line]
	local lo, hi = 1, line
	while lo < hi do
		local mid = (lo + hi) // 2
		if top[mid] >= t then
			hi = mid
		else
			lo = mid + 1
		end
	end
	return math.max(1, lo - 1)
end

-- What follows a folded line's text.
local function summary(r: Region): string
	local count = r.last - r.first
	local words = if count == 1 then "1 line" else `{count} lines`
	if r.closer then
		return `{M.ELLIPSIS}{r.closer}  {words}`
	end
	return `{M.ELLIPSIS} {words}`
end

---------------------------------------------------------------------------
-- The window: rows `from_row` to `to_row` of what shows, rounded out to whole
-- lines, as the text a textarea holds and the map back to the file.

function M.window(self: CodeView, from_row: number, to_row: number): Window
	local top, starts, text = self.top, self.starts, self.text
	local n = self.n
	local line = self:line_at(math.max(0, from_row))
	local total = self:total_rows()
	to_row = math.min(to_row, total)
	local rows: { Row } = {}
	local at: { number } = {}
	local parts: { string } = {}
	local count, used = 0, 0
	local summaries: { [string]: number }, wrapped: { [string]: { any } } = {}, {}
	local spacers: { [number]: number } = {}
	for _, block in self.blocks do
		spacers[block.line] = (spacers[block.line] or 0) + block.rows
	end
	local function push(row: Row, s: string)
		count += 1
		rows[count] = row
		at[count] = used
		parts[count] = s
		used += #s + 1
	end
	local first_line = line
	local last_line = line
	while line <= n and top[line] < to_row do
		last_line = line
		local a = starts[line]
		local e = self:line_end(line)
		local folded = self.folded[line]
		local b = self.breaks[line]
		if b then
			local from = 0
			local whole = string.sub(text, a + 1, e)
			for i = 1, #b + 1 do
				local to = b[i] or (e - a)
				local s = string.sub(whole, from + 1, to)
				push({ line = line, from = a + from, to = a + to, extra = 0, more = i > 1 }, s)
				wrapped[s] = { whole, from, to, i == #b + 1 }
				from = to
			end
		else
			local s = string.sub(text, a + 1, e)
			if folded then
				local tail = summary(folded)
				summaries[s .. tail] = #s
				push({ line = line, from = a, to = e, extra = #tail }, s .. tail)
			else
				push({ line = line, from = a, to = e, extra = 0 }, s)
			end
		end
		for _ = 1, spacers[line] or 0 do
			push({ line = line, from = e, to = e, extra = 0, spacer = true }, "")
		end
		line += 1
		if folded then
			line = folded.last + 1
		end
	end
	local win: Window = {
		first_row = top[first_line],
		rows = rows,
		at = at,
		text = table.concat(parts, "\n"),
		first_line = first_line,
		last_line = last_line,
		summaries = summaries,
		wrapped = wrapped,
	}
	self.win = win
	return win
end

-- The row of the window holding window offset `w`.
local function row_at(win: Window, w: number): number
	local at = win.at
	local lo, hi = 1, #at
	while lo < hi do
		local mid = (lo + hi + 1) // 2
		if at[mid] <= w then
			lo = mid
		else
			hi = mid - 1
		end
	end
	return lo
end
M.row_at = row_at

-- The file offset of window offset `w`. In what isn't the file (a summary,
-- a spacer row) it falls to the file's text before, or with `after`, the
-- text after.
function M.to_file(self: CodeView, w: number, after: boolean?): number
	local win = self.win :: Window
	local k = row_at(win, w)
	local r = win.rows[k]
	if r.spacer then
		if after then
			while k < #win.rows and win.rows[k].spacer do
				k += 1
			end
			local nxt = win.rows[k]
			return if nxt.spacer then #self.text else nxt.from
		end
		while k > 1 and win.rows[k].spacer do
			k -= 1
		end
		return win.rows[k].to
	end
	return r.from + math.min(w - win.at[k], r.to - r.from)
end

-- The window offset of file offset `p`, on the row it shows on; nil if that
-- isn't in the window. A place in a fold is its head's end.
function M.to_window(self: CodeView, p: number): number?
	local win = self.win
	if not win then
		return nil
	end
	local line = self:line_of(p)
	if self:is_hidden(line) then
		line = self:shown_line(line)
		p = self:line_end(line)
	end
	if line < win.first_line or line > win.last_line then
		return nil
	end
	local k = self.top[line] - win.first_row + 1
	local rows = win.rows
	while rows[k + 1] and rows[k + 1].more and rows[k + 1].line == line and rows[k + 1].from <= p do
		k += 1
	end
	return win.at[k] + (p - rows[k].from)
end

-- As `to_window`, but a place outside the window lands on its nearer end.
function M.to_window_clamped(self: CodeView, p: number): number
	local w = self:to_window(p)
	if w then
		return w
	end
	local win = self.win :: Window
	return if self:line_of(p) < win.first_line then 0 else #win.text
end

return M
