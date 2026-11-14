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

local tokens = require("@self/mention_tokens")

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

function M.end_token(s: any): tokens.Token?
	if not native(s) then
		local text: string = s:get()
		return tokens.token(text, #text)
	end
	return M.token(s, s:len())
end

return M
