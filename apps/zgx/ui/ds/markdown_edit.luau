-- Markdown as it is typed, as Zeron's composer edits it
-- (crates/ui/src/composer_markdown.rs). The draft stays plain markdown;
-- this only says how to show it and how a line break continues it.
--
-- `highlight(line, state)` is a textarea's `highlight`: per line, spans of
-- `{ from, to, color, weight?, italic?, strike?, bg? }` (bytes from 1, `to`
-- the last). The markdown itself stays in view, greyed: emphasis markers,
-- backticks, a heading's `#`s, a list's or quote's marker, fences and rules.
-- Bold, italic, struck and code text take their face. The state is 1 inside
-- a fenced code block, else 0. (A zgx without the richer spans draws the
-- colours alone, and ignores the rest.)
--
-- `newline(text, a, b)` is what Shift+Enter does in a list: the next item's
-- marker after the break, or, on an empty item, the item undone (outdented
-- a level). Bytes from 0, as an input's selection gives them.

local M = {}

local MARKER = "$text.tertiary"
local CODE_WASH = "$zeron.code.wash"

type Props = { [string]: any }
type Mark = { a: number, b: number, p: Props }

-- What a list line starts with: its indent, its marker, the space after it
-- (and a task box), where the item's text starts, and the next item's
-- marker.
type Prefix = { indent: string, marker: string, stop: number, next: string, number: boolean }
local function prefix(line: string): Prefix?
	local indent, bullet, space = string.match(line, "^(%s*)([-*+])(%s+)")
	if indent and bullet and space then
		local stop = #indent + #bullet + #space
		local box = string.match(line, "^%[[ xX]%]%s+", stop + 1)
		if box then
			stop += #box
		end
		return {
			indent = indent,
			marker = bullet,
			stop = stop,
			next = bullet .. space .. (if box then "[ ] " else ""),
			number = false,
		}
	end
	local digits: string?, close: string?
	indent, digits, close, space = string.match(line, "^(%s*)(%d+)([.)])(%s+)")
	if indent and digits and close and space then
		return {
			indent = indent,
			marker = digits .. close,
			stop = #indent + #digits + #close + #space,
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

-- Inline faces from `from` on: code spans first (nothing inside them is
-- markdown), then ~~strike~~, **bold** / __bold__, *italic* / _italic_.
-- Each found pair's markers are masked out of a copy of the line so the
-- next pass doesn't take them again.
local function inline(line: string, from: number, marks: { Mark })
	local function mark(a: number, b: number, p: Props)
		if b >= a then
			table.insert(marks, { a = a, b = b, p = p })
		end
	end
	local masked = line
	local function mask(a: number, b: number)
		masked = string.sub(masked, 1, a - 1) .. string.rep("\1", b - a + 1) .. string.sub(masked, b + 1)
	end
	-- Code: a run of backticks closed by the same run.
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
		mark(s, e, { color = MARKER })
		mark(e + 1, cs - 1, { bg = CODE_WASH })
		mark(cs, ce, { color = MARKER })
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
				s.weight, s.italic, s.strike, s.bg = p.weight, p.italic, p.strike, p.bg
				table.insert(out, span)
			end
		end
		last = c
	end
	return out
end

function M.highlight(line: string, state: number): ({ { any } }, number)
	local n = #line
	local fence = string.match(line, "^%s*```") or string.match(line, "^%s*~~~")
	if state ~= 0 then
		if fence then
			return { { 1, math.max(n, 1), MARKER } }, 0
		end
		if n == 0 then
			return {}, state
		end
		return { { 1, n, "$text.primary", bg = CODE_WASH } :: any }, state
	end
	if fence then
		return { { 1, n, MARKER } }, 1
	end
	if n == 0 then
		return {}, 0
	end
	if is_rule(line) then
		return { { 1, n, MARKER } }, 0
	end
	local marks: { Mark } = {}
	local from = 1
	-- A heading: its #s greyed, its text bold.
	local indent, hashes, space = string.match(line, "^(%s*)(#+)(%s+)")
	if indent and hashes and space and #hashes <= 6 then
		local stop = #indent + #hashes + #space
		table.insert(marks, { a = #indent + 1, b = stop, p = { color = MARKER } })
		table.insert(marks, { a = stop + 1, b = n, p = { weight = 600 } })
		from = stop + 1
	else
		-- A quote's > in the marker's ink.
		local q, gt = string.match(line, "^(%s*)(>%s?)")
		if q and gt then
			table.insert(marks, { a = #q + 1, b = #q + #gt, p = { color = MARKER } })
			from = #q + #gt + 1
		end
		local p = prefix(string.sub(line, from))
		if p then
			local at = from - 1 + #p.indent
			if p.number then
				table.insert(marks, { a = at + 1, b = at + #p.marker, p = { color = MARKER } })
			else
				table.insert(marks, { a = at + 1, b = at + 1, p = { color = MARKER } })
			end
			from = from + p.stop
		end
	end
	inline(line, from, marks)
	return runs(marks), 0
end

-- What a line break does at the selection `a`..`b` (bytes from 0) in a
-- list item: `(from, to, text)` to put in place of `from`..`to`, or nil
-- for a plain break.
function M.newline(text: string, a: number, b: number): (number?, number?, string?)
	if a ~= b then
		return nil, nil, nil
	end
	-- The caret's line, in bytes from 0.
	local start = 0
	local i = 0
	while true do
		local nl = string.find(text, "\n", i + 1, true)
		if not nl or nl > a then
			break
		end
		start = nl
		i = nl
	end
	local stop = string.find(text, "\n", a + 1, true)
	local line_end = if stop then stop - 1 else #text
	local line = string.sub(text, start + 1, line_end)
	-- Not inside a fenced code block: count the fences above.
	local fences = 0
	for fence in string.gmatch(string.sub(text, 1, start), "[^\n]*") do
		if string.match(fence, "^%s*```") or string.match(fence, "^%s*~~~") then
			fences += 1
		end
	end
	if fences % 2 == 1 then
		return nil, nil, nil
	end
	local p = prefix(line)
	if not p or a < start + p.stop then
		return nil, nil, nil
	end
	if string.match(string.sub(line, p.stop + 1), "^%s*$") then
		-- An empty item: undone, a level out.
		return start, line_end, string.sub(p.indent, 1, math.max(0, #p.indent - 2))
	end
	return a, a, "\n" .. p.indent .. p.next
end

return M
