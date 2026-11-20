-- What a prompt's words say of what it carries: Zeron's canonical
-- attachment mentions (crates/proto attachment_mentions.rs), as the shipping
-- app, mobile and the engine read them. A draft's attachment is referred to
-- by its number in that draft, never a path: a picture as
-- `[Image N](zeron-image:N)`, any other file as `[<its name>](zeron-attachment:N)`
-- (the name escaped as file_mentions.rs `escape_mention_label` escapes it).
-- The engine gives each agent the plain label ("Image 2", "notes.md"); the
-- message keeps the link, so it's a chip wherever it's shown, after a
-- relaunch too. In words alone (no part of the window), for the tests.

local M = {}

M.IMAGE = "zeron-image:"
M.FILE = "zeron-attachment:"

export type Mention = {
	-- Where its link runs in the words, bytes from 1, both ends in it.
	from: number,
	to: number,
	index: number,
	label: string,
	image: boolean,
}

-- What a picture is called in a draft: `Image N`.
function M.image_label(index: number): string
	return `Image {index}`
end

-- A name as a link's label takes it: `\`, `[`, `]` and a backtick escaped.
function M.escape(label: string): string
	return (string.gsub(label, "[\\%[%]`]", function(c: string): string
		return "\\" .. c
	end))
end

-- The link for attachment `index`: a picture's with no `name`.
function M.link(index: number, name: string?): string
	if name == nil then
		return `[{M.image_label(index)}]({M.IMAGE}{index})`
	end
	return `[{M.escape(name)}]({M.FILE}{index})`
end

-- A label as escaping makes it, undone; nil for any other (a bare `[`, `]`
-- or backtick, a control character, an escape of anything else, empty).
local function unescape(raw: string): string?
	local out: { string } = {}
	local i = 1
	while i <= #raw do
		local c = string.sub(raw, i, i)
		if c == "\\" then
			local nxt = string.sub(raw, i + 1, i + 1)
			if nxt == "\\" or nxt == "[" or nxt == "]" or nxt == "`" then
				table.insert(out, nxt)
				i += 2
			else
				return nil
			end
		elseif c == "[" or c == "]" or c == "`" or string.match(c, "%c") then
			return nil
		else
			table.insert(out, c)
			i += 1
		end
	end
	local label = table.concat(out)
	return if label ~= "" then label else nil
end

-- The canonical mentions in `text`, in order. Only links exactly as `link`
-- writes them count: escaped text, a link in code, or one with anything
-- else in it is left alone.
function M.mentions(text: string): { Mention }
	local out: { Mention } = {}
	if not string.find(text, M.IMAGE, 1, true) and not string.find(text, M.FILE, 1, true) then
		return out
	end
	-- (Inside code, nothing: a fence's or a span's.)
	local code: { { number } } = {}
	for s, e in string.gmatch(text, "()```.-```()") do
		table.insert(code, { s :: number, (e :: number) - 1 })
	end
	for s, e in string.gmatch(text, "()`[^`\n]+`()") do
		table.insert(code, { s :: number, (e :: number) - 1 })
	end
	local function in_code(at: number): boolean
		for _, r in code do
			if at >= r[1] and at <= r[2] then
				return true
			end
		end
		return false
	end
	local at = 1
	while true do
		local open = string.find(text, "[", at, true)
		if not open then
			break
		end
		-- (Escaped, `\[`: text, not a link. An odd run of backslashes.)
		local slashes = 0
		while open - slashes > 1 and string.sub(text, open - slashes - 1, open - slashes - 1) == "\\" do
			slashes += 1
		end
		if slashes % 2 == 1 then
			at = open + 1
			continue
		end
		-- (Its label: up to the first `](` not escaped.)
		local i = open + 1
		local close: number? = nil
		while i <= #text do
			local c = string.sub(text, i, i)
			if c == "\\" then
				i += 2
			elseif c == "]" then
				close = i
				break
			elseif c == "\n" then
				break
			else
				i += 1
			end
		end
		if not close then
			at = open + 1
			continue
		end
		local scheme_at = close + 2
		local is_image = string.sub(text, close + 1, close + 1 + #M.IMAGE) == "(" .. M.IMAGE
		local is_file = string.sub(text, close + 1, close + 1 + #M.FILE) == "(" .. M.FILE
		if not (is_image or is_file) or (open > 1 and string.sub(text, open - 1, open - 1) == "!") or in_code(open) then
			at = open + 1
			continue
		end
		local digits_at = scheme_at + (if is_image then #M.IMAGE else #M.FILE)
		local digits = string.match(text, "^(%d+)%)", digits_at)
		local index = if digits then tonumber(digits) else nil
		if not digits or not index or index < 1 or string.sub(digits, 1, 1) == "0" then
			at = open + 1
			continue
		end
		local to = digits_at + #digits
		local raw = string.sub(text, open + 1, close - 1)
		local label: string? = nil
		if is_image then
			label = if raw == M.image_label(index) then raw else nil
		else
			label = unescape(raw)
		end
		if label then
			table.insert(out, { from = open, to = to, index = index, label = label, image = is_image })
			at = to + 1
		else
			at = open + 1
		end
	end
	return out
end

-- `text` with each mention `plain` picks as its plain label.
local function replace(text: string, plain: (index: number) -> boolean): string
	local out: { string } = {}
	local at = 1
	for _, m in M.mentions(text) do
		if plain(m.index) then
			table.insert(out, string.sub(text, at, m.from - 1))
			table.insert(out, m.label)
			at = m.to + 1
		end
	end
	table.insert(out, string.sub(text, at))
	return table.concat(out)
end

-- The words as an agent reads them: each mention its plain label.
function M.prompt(text: string): string
	return replace(text, function(): boolean
		return true
	end)
end

-- A mention of an attachment no longer attached (`attached`: the numbers
-- that are) leaves as its plain label, never a link to nothing.
function M.demote_unattached(text: string, attached: { number }): string
	return replace(text, function(index: number): boolean
		return table.find(attached, index) == nil
	end)
end

-- An upload's name as it's shown: uploads are stored as `{id8}-{name}`.
function M.display_name(name: string): string
	local id, rest = string.match(name, "^(%x%x%x%x%x%x%x%x)%-(.+)$")
	return if id and rest then rest else name
end

-- The draft number of an upload or staged picture named `Image 2.png`, or
-- `ab12cd34-Image_2.png` (uploads put `_` for spaces); nil for another.
function M.image_index(name: string): number?
	local base = string.match(name, "([^/\\]+)$") or name
	local stem = string.match(base, "^(.*)%.[^%.]*$") or base
	local head, digits = string.match(stem, "^(.-)[ _](%d+)$")
	if not head or not digits then
		return nil
	end
	if head ~= "Image" and not string.match(head, "%-Image$") then
		return nil
	end
	local n = tonumber(digits)
	return if n and n > 0 then n else nil
end

-- A name as the engine keeps it in an upload's: what isn't an ASCII letter,
-- digit, `.`, `-` or `_` an `_` (each character, whatever its bytes).
function M.sanitized(name: string): string
	local out: { string } = {}
	for _, code in utf8.codes(name) do
		local c = utf8.char(code)
		if string.match(c, "^[%w%.%-_]$") and code < 128 then
			table.insert(out, c)
		else
			table.insert(out, "_")
		end
	end
	return table.concat(out)
end

-- Whether a mention is the attachment at `path`: a picture by its number,
-- a file by its name as the engine sanitizes it.
function M.names(m: Mention, path: string): boolean
	if m.image then
		return M.image_index(path) == m.index
	end
	local base = string.match(path, "([^/\\]+)$") or path
	return M.sanitized(m.label) == M.sanitized(M.display_name(base))
end

return M
