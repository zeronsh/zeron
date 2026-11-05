-- Where a file folds: the regions a code editor's gutter offers, found from
-- its shape, not parsed. Indentation says most of it: a line with deeper
-- lines under it opens a region, which the next line back at its depth ends.
-- A closer there (a }, an `end`, a </div>) goes with it, so a folded function
-- reads `fn add(a, b) { ⋯ }`; a closer that opens again (`} else {`) stays
-- out, the head of a region of its own. Runs of comments and of imports fold
-- too, three lines or more, and a Markdown file folds by its headings.
--
-- `Folds.scan(text, language, comment, pause)` walks a file once, about half
-- a microsecond a line, and gives the regions in order, each `{ first, last,
-- closer }`: lines from 1, `closer` what the folded line ends with. `pause`, if
-- given, is called every few hundred lines (a coroutine yields there, so a
-- long file never holds up a frame).

local M = {}

export type Region = {
	first: number,
	last: number,
	-- The closing line's text, trimmed, when `last` is one.
	closer: string?,
}

-- Runs shorter than this stay open: two imports aren't worth a control.
local RUN = 3
-- How wide a tab counts, comparing depths.
local TAB = 4
-- Lines between calls to `pause`.
local STRIDE = 256

-- Words that end a block, at the start of a line.
local WORDS: { [string]: boolean } = {
	["end"] = true,
	["until"] = true,
	["else"] = true,
	["elseif"] = true,
	["elif"] = true,
	["fi"] = true,
	["done"] = true,
	["esac"] = true,
}

-- Whether a line (from its first character) closes what is open.
local function closes(body: string): boolean
	local b = string.byte(body, 1)
	-- } ) ]
	if b == 125 or b == 41 or b == 93 then
		return true
	end
	-- </
	if b == 60 and string.byte(body, 2) == 47 then
		return true
	end
	local word = string.match(body, "^%a+")
	return word ~= nil and WORDS[word] == true
end

-- Whether a line ends opening something, so the block goes on under it.
local function opens(body: string): boolean
	local last = string.match(body, "(%S)%s*$")
	if last == "{" or last == "(" or last == "[" or last == ":" then
		return true
	end
	return string.find(body, "%f[%w_]then%s*$") ~= nil
		or string.find(body, "%f[%w_]do%s*$") ~= nil
		or string.find(body, "%f[%w_]else%s*$") ~= nil
end

local function trimmed(s: string): string
	return (string.gsub(s, "%s+$", ""))
end

-- Lines an import run is made of.
local function imports(body: string): boolean
	local word = string.match(body, "^[%a#_]+")
	return word == "use"
		or word == "import"
		or word == "#include"
		or word == "from"
		or word == "require"
		or (word == "local" and string.find(body, "=%s*require%(") ~= nil)
		or (word == "pub" and string.find(body, "^pub use ") ~= nil)
		or (word == "const" and string.find(body, "=%s*require%(") ~= nil)
end

-- A Markdown file: each heading to the next of its level or above.
local function headings(text: string, pause: (() -> ())?): { Region }
	local out: { Region } = {}
	local open: { { level: number, line: number } } = {}
	local line = 0
	local fenced = false
	local last_text = 0
	local function close(level: number)
		while #open > 0 and open[#open].level >= level do
			local top = table.remove(open) :: { level: number, line: number }
			if last_text > top.line then
				table.insert(out, { first = top.line, last = last_text })
			end
		end
	end
	for row in string.gmatch(text, "([^\n]*)\n?") do
		line += 1
		if pause and line % STRIDE == 0 then
			pause()
		end
		if string.find(row, "^%s*```") or string.find(row, "^%s*~~~") then
			fenced = not fenced
		end
		local marks = if fenced then nil else string.match(row, "^(#+) ")
		if marks then
			close(#marks)
			table.insert(open, { level = #marks, line = line })
		end
		if string.find(row, "%S") then
			last_text = line
		end
	end
	close(1)
	table.sort(out, function(a: Region, b: Region): boolean
		return a.first < b.first
	end)
	return out
end

function M.scan(text: string, language: string?, comment: string?, pause: (() -> ())?): { Region }
	if language == "markdown" then
		return headings(text, pause)
	end
	local out: { Region } = {}
	-- What is open, deepest last: each a line and its depth.
	local lines: { number } = {}
	local depths: { number } = {}
	local top = 0
	local line = 0
	-- The last line with anything on it.
	local last = 0
	-- The run of comments or imports going on: its kind, start, depth, end.
	local run_kind: string? = nil
	local run_first, run_depth, run_last = 0, 0, 0
	local function end_run()
		if run_kind and run_last - run_first + 1 >= RUN then
			table.insert(out, { first = run_first, last = run_last })
		end
		run_kind = nil
	end
	local pad = if comment then #comment else 0
	for lead, body in string.gmatch(text, "([ \t]*)([^\n]*)\n?") do
		line += 1
		if pause and line % STRIDE == 0 then
			pause()
		end
		-- (gmatch types its second capture as optional; it always matches.)
		if body == nil or body == "" then
			continue
		end
		local depth = #lead
		if string.find(lead, "\t", 1, true) then
			local _, tabs = string.gsub(lead, "\t", "")
			depth += tabs * (TAB - 1)
		end
		local is_comment = comment ~= nil and string.sub(body, 1, pad) == comment
		local kind = if is_comment then "comment" elseif imports(body) then "import" else nil
		if kind ~= run_kind or depth ~= run_depth or line ~= run_last + 1 then
			end_run()
			if kind then
				run_kind, run_first, run_depth = kind, line, depth
			end
		end
		if kind then
			run_last = line
		end
		-- What this line is no deeper than ends here. A closer at the depth
		-- of what it closes goes with it, unless it opens again.
		local closer = closes(body)
		while top > 0 and depths[top] >= depth do
			local first = lines[top]
			top -= 1
			if closer and depths[top + 1] == depth and first < last then
				if opens(body) then
					table.insert(out, { first = first, last = last })
				else
					table.insert(out, { first = first, last = line, closer = trimmed(body) })
				end
				closer = false
			elseif last > first then
				table.insert(out, { first = first, last = last })
			end
		end
		top += 1
		lines[top], depths[top] = line, depth
		last = line
	end
	end_run()
	while top > 0 do
		if last > lines[top] then
			table.insert(out, { first = lines[top], last = last })
		end
		top -= 1
	end
	table.sort(out, function(a: Region, b: Region): boolean
		return a.first < b.first
	end)
	-- One region a line: the outer of two starting together.
	local kept: { Region } = table.create(#out) :: { Region }
	for _, r in out do
		local prior = kept[#kept]
		if prior and prior.first == r.first then
			if r.last > prior.last then
				kept[#kept] = r
			end
		else
			table.insert(kept, r)
		end
	end
	return kept
end

return M
