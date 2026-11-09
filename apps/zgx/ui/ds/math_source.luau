-- Markdown's math, read before markdown is (ds/math re-exports it as
-- `math.prepare`, ds/markdown runs it). Plain Luau, nothing of the
-- interface's, so a test reads it directly.

local M = {}

-- Markdown's math, before markdown reads it: outside code (fences and
-- inline code), `$$tex$$` becomes a ```math block (kept literal: CommonMark
-- would take `\\` and `\{` as escapes), and inline `$tex$` becomes inline
-- code showing `$tex$`, by pulldown-cmark's rules (no space just inside the
-- dollars, a closing one not followed by a digit, so "$5 and $10" stays
-- text). `\$` is a dollar. A `$$` still open (a reply streaming in) is left
-- as it is until it closes.
local BACKTICK, TILDE, DOLLAR, SLASH, NEWLINE, SPACE, TAB, BRACKET = 96, 126, 36, 92, 10, 32, 9, 91
-- a sent prompt's skills are labelled with their `$` (`[$review](zeron-invoke:…)`),
-- so two in one message would otherwise read as one formula between them
local ZERON_LINK = "^%[[^%]\n]*%]%(zeron%-[%a]+:[^%)%s]*%)"
-- The bytes the read stops at; any other is copied as it is.
local SPECIAL = "[\n\\%[`%$]"
-- A line that may act on the text around it, as markdown reads it: a link's
-- definition (a reference anywhere takes it), or HTML (a comment or `<pre>`
-- runs on past blank lines). Inside a list or a quote too.
local DEFINITION = "^[ \t>%-%+%*%d%.%)]*%[[^%]\n]+%]:"
local HTML = "^[ \t>%-%+%*%d%.%)]*<[%a!%?/]"

local function blank(b: number?): boolean
	return b == nil or b == SPACE or b == TAB or b == NEWLINE
end

local function digit(b: number?): boolean
	return b ~= nil and b >= 48 and b <= 57
end

-- Whether a line after a blank one starts a block of its own, whatever came
-- before it: a word (a paragraph), a heading, a fence, math, a table, a
-- picture. Not a list's marker or a number (`2.` goes on with a list), a
-- quote's, an indent, HTML or a link's definition.
local function starts_block(b: number?): boolean
	if b == nil then
		return false
	end
	return (b >= 65 and b <= 90)
		or (b >= 97 and b <= 122)
		or b >= 128
		or b == 35 -- #
		or b == BACKTICK
		or b == TILDE
		or b == DOLLAR
		or b == 33 -- !
		or b == 34 -- "
		or b == 39 -- '
		or b == 40 -- (
		or b == 124 -- |
end

-- `tex` as inline code (its fence longer than any run of backticks in it).
local function inline_code(tex: string): string
	local longest = 0
	for run in string.gmatch(tex, "`+") do
		longest = math.max(longest, #run)
	end
	local fence = string.rep("`", longest + 1)
	local body = `${tex}$`
	return `{fence}{body}{fence}`
end

-- A read of a text as it grows (a reply streaming in): where it got to, so
-- the next, longer text is read on from there, not from its start.
--
-- The read goes left to right, and some of what it decides looks ahead (an
-- inline `$` to the one closing it, a fence to its close, inline code to its
-- closing run). It keeps the last place (`i`) before which nothing it
-- decided looked at a byte at or past it: text added at the end can't
-- change what it made of the text up to there (`out`, the pieces made, and
-- `from`, where the copying as it is began). Two kinds of looking ahead
-- don't hold it back:
-- - inline code with no closing run in a paragraph that's over (`ticks`, a
--   stray backtick): it stays unclosed unless the text added brings one,
--   which the next read checks first (and starts over if it does);
-- - a fence still open at the end (`fence`): the next read goes on from its
--   last whole line.
--
-- It also marks where the text can be parsed in two (`cut`, `cut_at`): a
-- line after a blank one, past which nothing can join what's before it.
-- The pieces up to `cut` are what markdown reads before it, settled.
type Tick = { run: string, next: number }
type Fence = { i: number, mark: number, run: number, at: number }
export type Scan = {
	src: string,
	out: { string },
	-- the pieces made up to the place it goes on from, and all made by the
	-- last read
	on: number,
	made: number,
	i: number,
	from: number,
	-- what the last read made past its last piece: `src` from `from` on
	rest: string,
	ticks: { Tick },
	nt: number,
	fence: Fence?,
	-- the lines looked at for what acts at a distance: where the next one
	-- starts, and the fence they're in
	lines: number,
	line_fence: { mark: number, run: number }?,
	-- where the fences the lines found are (bytes, `to` huge while open),
	-- those past the head's cut: never cut inside one (the math read's own
	-- idea of a fence is its own: it pairs backticks across lines)
	fences: { { from: number, to: number } },
	cut: number,
	cut_at: number,
	-- a line that acts at a distance was seen: never parsed in two
	whole: boolean,
}

function M.scan(): Scan
	return {
		src = "",
		out = {},
		on = 0,
		made = 0,
		i = 1,
		from = 1,
		rest = "",
		ticks = {},
		nt = 0,
		fence = nil,
		lines = 1,
		line_fence = nil,
		fences = {},
		cut = 0,
		cut_at = 1,
		whole = false,
	}
end

-- Whether the line starting at `at` may act at a distance.
local function distant(src: string, at: number): boolean
	return string.find(src, DEFINITION, at) ~= nil or string.find(src, HTML, at) ~= nil
end

-- Read `st.src` on from where the last read could go on.
local function read(st: Scan)
	local src = st.src
	local n = #src
	local out = st.out
	-- (What the last read made past its place goes.)
	for k = st.on + 1, st.made do
		out[k] = nil
	end
	local on = st.on
	local i, from = st.i, st.from
	local ticks, nt = st.ticks, st.nt
	for k = nt + 1, #ticks do
		ticks[k] = nil
	end
	local was_fence = st.fence
	local fence: Fence? = nil
	local whole = st.whole
	local cut, cut_at = st.cut, st.cut_at
	-- Whether `at` is in a fence the lines found.
	local fences = st.fences
	local function fenced(at: number): boolean
		for _, f in fences do
			if at > f.from and at <= f.to then
				return true
			end
		end
		return false
	end
	-- The furthest byte looked at by what was decided so far.
	local reach = 0
	-- The last place it can go on from.
	local ci, cfrom, con, cnt = i, from, on, nt
	local function flush(to: number)
		if to >= from then
			on += 1
			out[on] = string.sub(src, from, to)
		end
	end
	while true do
		-- (A reach past the end: what was decided waits on text not there yet.)
		if reach < i and reach <= n then
			if
				not whole
				and i > 2
				and i <= n
				and string.byte(src, i - 1) == NEWLINE
				and string.byte(src, i - 2) == NEWLINE
				and starts_block(string.byte(src, i))
				and not fenced(i)
			then
				flush(i - 1)
				from = i
				cut, cut_at = on, i
			end
			ci, cfrom, con, cnt = i, from, on, nt
		end
		if i > n then
			break
		end
		local b = string.byte(src, i)
		if i == 1 or string.byte(src, i - 1) == NEWLINE then
			-- A fence (up to three spaces in): copied as it is, to its close.
			local lead = string.match(src, "^ ? ? ?", i) or ""
			local mark = string.byte(src, i + #lead)
			reach = math.max(reach, i + #lead)
			if mark == BACKTICK or mark == TILDE then
				local run = string.match(src, if mark == BACKTICK then "^`+" else "^~+", i + #lead) or ""
				reach = math.max(reach, i + #lead + #run)
				if #run >= 3 then
					local at: number?
					local open = was_fence
					if open and open.i == i and open.mark == mark and open.run == #run then
						-- (Its lines up to the last whole one aren't its close.)
						at = open.at
					else
						at = string.find(src, "\n", i, true)
						-- (A backtick in a backtick fence's info: CommonMark's
						-- paragraph, not a fence. Not parsed in two after it.)
						if at and mark == BACKTICK and string.find(string.sub(src, i + #lead + #run, at), "`", 1, true) then
							whole = true
							cut, cut_at = 0, 1
						end
					end
					local last = at
					while at do
						last = at
						local line = string.match(src, "^[^\n]*", at + 1) or ""
						local close = string.match(line, if mark == BACKTICK then "^ ? ? ?(`+)%s*$" else "^ ? ? ?(~+)%s*$")
						if close and #close >= #run then
							at = at + #line + 1
							break
						end
						at = string.find(src, "\n", at + 1, true)
					end
					if at then
						reach = math.max(reach, at)
						i = at
					else
						reach = n + 1
						if last then
							fence = { i = i, mark = mark, run = #run, at = last }
						end
						i = n + 1
					end
					continue
				end
			end
		end
		if b == SLASH then
			reach = math.max(reach, i)
			i += 2
		elseif b == BRACKET and string.match(src, ZERON_LINK, i) then
			i += #(string.match(src, ZERON_LINK, i) :: string)
			reach = math.max(reach, i - 1)
		elseif b == BRACKET then
			-- (Not one: the furthest the pattern looked is its `]` and the
			-- `)` or space after it, on this line.)
			local close = string.find(src, "[%]\n]", i + 1)
			if close and string.byte(src, close) ~= NEWLINE then
				close = string.find(src, "[%)%s]", close + 1)
			end
			reach = math.max(reach, close or n + 1)
			i = string.find(src, SPECIAL, i + 1) or n + 1
		elseif b == BACKTICK then
			-- Inline code: past its closing run of as many backticks.
			local run = string.match(src, "^`+", i) or "`"
			reach = math.max(reach, i + #run)
			local next = i + #run
			local close = string.find(src, run, next, true)
			while close and string.byte(src, close + #run) == BACKTICK do
				next = close + #run + 1
				close = string.find(src, run, next, true)
			end
			if close then
				i = close + #run
				reach = math.max(reach, i)
			else
				-- (Unclosed in a paragraph that's over: a stray one, which
				-- the next read checks again. In the last one, it's likely
				-- being written: the read goes on from before it.)
				if string.find(src, "\n\n", i + #run, true) then
					nt += 1
					ticks[nt] = { run = run, next = next }
				else
					reach = n + 1
				end
				i += #run
			end
		elseif b == DOLLAR then
			reach = math.max(reach, i + 1)
			if string.byte(src, i + 1) == DOLLAR then
				local close = string.find(src, "$$", i + 2, true)
				if not close then
					reach = n + 1
					break
				end
				local tex = string.match(string.sub(src, i + 2, close - 1), "^%s*(.-)%s*$") or ""
				flush(i - 1)
				local before = if on > 0 then out[on] else ""
				local lead = if before == "" or string.sub(before, -1) == "\n" then "" else "\n"
				local after = string.byte(src, close + 2)
				local trail = if after == nil or after == NEWLINE then "" else "\n"
				on += 1
				out[on] = `{lead}\`\`\`math\n{tex}\n\`\`\`{trail}`
				reach = math.max(reach, close + 2)
				i = close + 2
				from = i
			elseif not blank(string.byte(src, i + 1)) then
				-- Inline: to the next dollar on its paragraph that closes it.
				local j = i + 1
				local found = nil
				while j <= n do
					local c = string.byte(src, j)
					if c == SLASH then
						j += 2
					elseif c == DOLLAR then
						if not blank(string.byte(src, j - 1)) and not digit(string.byte(src, j + 1)) then
							found = j
						end
						break
					elseif c == NEWLINE and string.byte(src, j + 1) == NEWLINE then
						break
					else
						j += 1
					end
				end
				reach = math.max(reach, if j > n then n + 1 else j + 1)
				if found then
					flush(i - 1)
					on += 1
					out[on] = inline_code(string.sub(src, i + 1, found - 1))
					i = found + 1
					from = i
				else
					i += 1
				end
			else
				i += 1
			end
		else
			reach = math.max(reach, i)
			i = if b == NEWLINE then i + 1 else string.find(src, SPECIAL, i + 1) or n + 1
		end
	end
	st.made = on
	st.rest = if from <= n then string.sub(src, from, n) else ""
	st.i, st.from, st.on, st.nt = ci, cfrom, con, cnt
	st.fence = fence
	st.cut, st.cut_at, st.whole = cut, cut_at, whole
end

-- The text's lines, from where the last look stopped, for one that acts at
-- a distance (outside fences, which the lines are followed through as
-- markdown reads them: math and inline code don't matter to lines). The
-- last line, which may grow, is looked at as it is and again next time.
local function look_at_lines(st: Scan, src: string)
	local at = st.lines
	while not st.whole do
		local eol = string.find(src, "\n", at, true)
		local fence = st.line_fence
		if fence then
			if not eol then
				break
			end
			local line = string.sub(src, at, eol - 1)
			local close = string.match(line, if fence.mark == BACKTICK then "^ ? ? ?(`+)%s*$" else "^ ? ? ?(~+)%s*$")
			if close and #close >= fence.run then
				st.line_fence = nil
				st.fences[#st.fences].to = eol
			end
		else
			local run = string.match(src, "^ ? ? ?(`+)", at) or string.match(src, "^ ? ? ?(~+)", at)
			local info = if run and eol then string.sub(src, at, eol - 1) else ""
			if run and #run >= 3 and eol and not (string.byte(run, 1) == BACKTICK and string.find(info, "`", string.find(info, run, 1, true) :: number + #run, true)) then
				st.line_fence = { mark = string.byte(run, 1), run = #run }
				table.insert(st.fences, { from = at, to = math.huge })
			elseif distant(src, at) then
				st.whole = true
			end
			if not eol then
				break
			end
		end
		-- (A whole line: its end found, or the loop left above.)
		at = (eol :: number) + 1
	end
	st.lines = at
	if st.whole then
		st.cut, st.cut_at = 0, 1
	end
end

-- Whether inline code left unclosed before where the read goes on is still
-- unclosed with `src` (which goes on from the last read's text), each
-- looking on from where it stopped.
local function still_unclosed(st: Scan, src: string): boolean
	local seen = #st.src
	for k = 1, st.nt do
		local t = st.ticks[k]
		local next = math.max(t.next, seen - #t.run + 2)
		local close = string.find(src, t.run, next, true)
		while close and string.byte(src, close + #t.run) == BACKTICK do
			next = close + #t.run + 1
			close = string.find(src, t.run, next, true)
		end
		if close then
			return false
		end
		t.next = next
	end
	return true
end

-- Read `src`, the text as it is now. One that goes on from the last read's
-- is read from where that one could go on; any other from its start. True
-- when it started over (what was made and cut before is gone).
function M.feed(st: Scan, src: string): boolean
	local goes_on = #src >= #st.src and (st.src == "" or string.find(src, st.src, 1, true) == 1)
	local over = not goes_on or not still_unclosed(st, src)
	if over then
		table.clear(st :: any)
		for k, v in M.scan() :: any do
			(st :: any)[k] = v
		end
	end
	st.src = src
	look_at_lines(st, src)
	-- (The fences before the cut are behind it for good.)
	local fences = st.fences
	while #fences > 0 and fences[1].to < st.cut_at do
		table.remove(fences, 1)
	end
	read(st)
	return over
end

-- What the read made, from the piece after `piece` (0: all of it).
function M.since(st: Scan, piece: number): string
	if piece >= st.made then
		return st.rest
	end
	return table.concat(st.out, "", piece + 1, st.made) .. st.rest
end

function M.prepare(src: string): string
	if not string.find(src, "$", 1, true) then
		return src
	end
	local st = M.scan()
	st.src = src
	read(st)
	return M.since(st, 0)
end

return M
