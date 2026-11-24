-- A draft's text read where it's held (its state), not copied whole into the
-- script: what the composer's bindings need of it at every keystroke.
--
-- A state's `get` makes the whole draft a script string, and a dozen of the
-- composer's bindings each did at every keystroke: a key in a draft of
-- 30,000 lines cost 5 ms in copies alone. These read only the part they
-- need (zgx's `state:sub`, `state:len`, `state:find`), so a key costs the
-- same in a draft of 20 lines or 30,000. A zgx without `sub` reads the whole
-- draft, as before.
--
--   draft_text.track(s)          follow its edits (nothing read)
--   draft_text.size(s)           its length in bytes
--   draft_text.sub(s, i, j?)     `string.sub` of it
--   draft_text.has_text(s)       whether it holds anything but blanks
--   draft_text.multiline(s)      whether it has a line break
--   draft_text.token(s, caret)   the mention token the caret is in
--                                (ds/mention_tokens), or nil
--   draft_text.end_token(s)      the one at its end
--   draft_text.holds(s, text)    whether `text` is anywhere in it
--   draft_text.line_at(s, at)    the line a byte is in: where it starts,
--                                its text's start (a list's marker)
--   draft_text.fenced(s, at)     whether a byte is inside a fenced block

-- (By the `@ds` alias, not `@self`: the test runner resolves the one and
-- not the other.)
local tokens = require("@ds/mention_tokens")

local M = {}

-- Whether states read in place (`sub`, and `len` a string's length).
local in_place: boolean? = nil
local function native(s: any): boolean
	if in_place == nil then
		in_place = pcall(function()
			return s:sub(1, 0)
		end)
	end
	return in_place :: boolean
end

-- Follows its edits, as `get` does, without reading it.
function M.track(s: any)
	s:len()
end

function M.size(s: any): number
	return if native(s) then s:len() else #(s:get() :: string)
end

function M.sub(s: any, i: number, j: number?): string
	return if native(s) then s:sub(i, j) else string.sub(s:get(), i, j)
end

-- How much of its start shows whether it has text: past that many blanks,
-- the rest is read.
local HEAD = 256

function M.has_text(s: any): boolean
	if not native(s) then
		return string.find(s:get(), "%S") ~= nil
	end
	if string.find(s:sub(1, HEAD), "%S") then
		return true
	end
	return s:len() > HEAD and string.find(s:get(), "%S") ~= nil
end

function M.holds(s: any, text: string): boolean
	if not native(s) then
		return string.find(s:get(), text, 1, true) ~= nil
	end
	return (s:count(text, { limit = 1 })) > 0
end

function M.multiline(s: any): boolean
	return #s:find("\n", { limit = 1 }) > 0
end

-- How far either side of the caret a token is looked for first: none runs
-- this long (ds/mention_tokens' reach).
local NEAR = 1024

-- The token at `caret` (bytes from 0): looked for around the caret, and only
-- where there is one, in the whole draft (whether a fence above holds it).
-- The look around can't tell a fence's parity, though: a closed block
-- opened more than `NEAR` above shows it only its closing fence, which
-- reads as one still open. Where it found nothing with a fence before the
-- caret, the whole draft decides.
function M.token(s: any, caret: number): tokens.Token?
	if not native(s) then
		return tokens.token(s:get(), caret)
	end
	local lo = math.max(1, caret - NEAR + 1)
	local near = s:sub(lo, caret + NEAR)
	if tokens.token(near, caret - lo + 1) == nil then
		local fence = string.find(near, "```", 1, true)
		if fence == nil or fence > caret - lo + 1 then
			return nil
		end
	end
	return tokens.token(s:get(), caret)
end

-- How much of a line `line_at` reads: enough for any list's marker.
local HEAD_OF_LINE = 256

-- The line byte `at` (from 0) is in: where it starts (bytes from 0) and
-- its first `HEAD_OF_LINE` bytes. Nil when its start is past `NEAR` back
-- (a line that long has no list marker near the caret).
function M.line_at(s: any, at: number): (number?, string?)
	local lo = math.max(1, at - NEAR + 1)
	local before = M.sub(s, lo, at)
	local nl = string.match(before, "^.*\n()")
	local start: number
	if nl then
		start = lo + nl - 2
	elseif lo == 1 then
		start = 0
	else
		return nil, nil
	end
	local head = M.sub(s, start + 1, start + HEAD_OF_LINE)
	local stop = string.find(head, "\n", 1, true)
	return start, if stop then string.sub(head, 1, stop - 1) else head
end

-- Whether byte `at` (from 0) is inside a fenced block: an odd number of
-- the lines before it start with ``` or ~~~ (after blanks), found where the
-- draft is held, each fence's line start read alone.
function M.fenced(s: any, at: number): boolean
	if not native(s) then
		local text: string = s:get()
		local count = 0
		for _, fence in { "```", "~~~" } do
			local from = 1
			while true do
				local i = string.find(text, fence, from, true)
				if i == nil or i > at then
					break
				end
				local lead = string.sub(text, math.max(1, i - 64), i - 1)
				if string.match(lead, "\n[ \t]*$") or (i <= 65 and string.match(lead, "^[ \t]*$")) then
					count += 1
				end
				from = i + #fence
			end
		end
		return count % 2 == 1
	end
	local count = 0
	for _, fence in { "```", "~~~" } do
		local found = s:find(fence, { limit = 4096 })
		for _, hit in found do
			local i = hit[1]
			if i > at then
				break
			end
			local lead = M.sub(s, math.max(1, i - 64), i - 1)
			if string.match(lead, "\n[ \t]*$") or (i <= 65 and string.match(lead, "^[ \t]*$")) then
				count += 1
			end
		end
	end
	return count % 2 == 1
end

function M.end_token(s: any): tokens.Token?
	if not native(s) then
		local text: string = s:get()
		return tokens.token(text, #text)
	end
	return M.token(s, s:len())
end

return M
