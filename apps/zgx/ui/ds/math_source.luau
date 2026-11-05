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
local BACKTICK, TILDE, DOLLAR, SLASH, NEWLINE, SPACE, TAB = 96, 126, 36, 92, 10, 32, 9

local function blank(b: number?): boolean
	return b == nil or b == SPACE or b == TAB or b == NEWLINE
end

local function digit(b: number?): boolean
	return b ~= nil and b >= 48 and b <= 57
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

function M.prepare(src: string): string
	if not string.find(src, "$", 1, true) then
		return src
	end
	local out: { string } = {}
	local n = #src
	local i = 1
	local from = 1 -- the start of what's copied as it is
	local function flush(to: number)
		if to >= from then
			table.insert(out, string.sub(src, from, to))
		end
	end
	local function line_start(at: number): boolean
		return at == 1 or string.byte(src, at - 1) == NEWLINE
	end
	while i <= n do
		local b = string.byte(src, i)
		if line_start(i) then
			-- A fence (up to three spaces in): copied as it is, to its close.
			local lead = string.match(src, "^ ? ? ?", i) or ""
			local mark = string.byte(src, i + #lead)
			if mark == BACKTICK or mark == TILDE then
				local run = string.match(src, if mark == BACKTICK then "^`+" else "^~+", i + #lead) or ""
				if #run >= 3 then
					local at = string.find(src, "\n", i, true)
					while at do
						local line = string.match(src, "^[^\n]*", at + 1) or ""
						local close = string.match(line, if mark == BACKTICK then "^ ? ? ?(`+)%s*$" else "^ ? ? ?(~+)%s*$")
						if close and #close >= #run then
							at = at + #line + 1
							break
						end
						at = string.find(src, "\n", at + 1, true)
					end
					i = if at then at else n + 1
					continue
				end
			end
		end
		if b == SLASH then
			i += 2
		elseif b == BACKTICK then
			-- Inline code: past its closing run of as many backticks.
			local run = string.match(src, "^`+", i) or "`"
			local close = string.find(src, run, i + #run, true)
			while close and string.byte(src, close + #run) == BACKTICK do
				close = string.find(src, run, close + #run + 1, true)
			end
			i = if close then close + #run else i + #run
		elseif b == DOLLAR and string.byte(src, i + 1) == DOLLAR then
			local close = string.find(src, "$$", i + 2, true)
			if not close then
				break
			end
			local tex = string.match(string.sub(src, i + 2, close - 1), "^%s*(.-)%s*$") or ""
			flush(i - 1)
			local before = if #out > 0 then out[#out] else ""
			local lead = if before == "" or string.sub(before, -1) == "\n" then "" else "\n"
			local after = string.byte(src, close + 2)
			local trail = if after == nil or after == NEWLINE then "" else "\n"
			table.insert(out, `{lead}\`\`\`math\n{tex}\n\`\`\`{trail}`)
			i = close + 2
			from = i
		elseif b == DOLLAR and not blank(string.byte(src, i + 1)) then
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
			if found then
				flush(i - 1)
				table.insert(out, inline_code(string.sub(src, i + 1, found - 1)))
				i = found + 1
				from = i
			else
				i += 1
			end
		else
			i += 1
		end
	end
	flush(n)
	return table.concat(out)
end

return M
