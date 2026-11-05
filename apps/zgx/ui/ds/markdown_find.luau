-- Find in a Markdown text, by its words as drawn (not its source): a
-- block's text parts in the order ds/markdown draws them, and where a query
-- falls in each. The chat's find counts with these (screens/chat/find), and
-- ds/markdown marks with them, so a match's number is the same on both
-- sides: the `k`th in its top-level block, counting through the block's
-- paragraphs (a heading's, a list's items', a quote's, a table's cells
-- column by column, a code block's lines) in that order.
--
-- A query never spans two parts (a paragraph and the next), and case is
-- ignored.

local prepare = require("@self/math_source").prepare

local M = {}

-- Pictures that follow one another (blocks of their own, nothing between:
-- ui.markdown makes one of each picture on a line of its own) as one
-- block, `{ t = "pics", list = { … } }`, drawn together on a tray. The same
-- pictures give the very same table each time (kept by the first one's
-- table), so a reply being written doesn't build its pictures again with
-- every word.
local trays: { [any]: { block: any, list: { any } } } = setmetatable({}, { __mode = "k" }) :: any

local function same(a: { any }, b: { any }): boolean
	if #a ~= #b then
		return false
	end
	for i, v in a do
		if b[i] ~= v then
			return false
		end
	end
	return true
end

local function together(list: { MarkdownBlock }): { MarkdownBlock }
	local out: { MarkdownBlock }? = nil
	local i = 1
	while i <= #list do
		local b = list[i]
		local j = i
		while list[j + 1] and list[j + 1].t == "img" do
			j += 1
		end
		if b.t == "img" and j > i then
			out = out or table.move(list, 1, i - 1, 1, {})
			local members = table.move(list, i, j, 1, {})
			local kept = trays[b]
			if not kept or not same(kept.list, members) then
				kept = { block = { t = "pics", list = members }, list = members }
				trays[b] = kept
			end
			table.insert(out :: any, kept.block)
			i = j + 1
		else
			if out then
				table.insert(out, b)
			end
			i += 1
		end
	end
	return out or list
end

-- A text's top-level blocks, as ds/markdown draws them (it parses with
-- this): its math read first (ds/math_source), pictures together one block.
-- `opts`: ui.markdown's (`breaks`: a line break is a break, as a prompt's
-- are), or `breaks` alone.
function M.parse(text: string, opts: (boolean | MarkdownOptions)?): { MarkdownBlock }
	local o: MarkdownOptions = if type(opts) == "table" then opts else { breaks = opts == true }
	return together(ui.markdown(prepare(text), o))
end

-- Where `q` (lower case, not empty) falls in `text`: each match's first and
-- last byte, from the start, not overlapping.
function M.occurrences(text: string, q: string): { { number } }
	local out: { { number } } = {}
	if q == "" then
		return out
	end
	local hay = string.lower(text)
	local at = 1
	while true do
		local s, e = string.find(hay, q, at, true)
		if not s or not e then
			break
		end
		table.insert(out, { s, e })
		at = e + 1
	end
	return out
end

-- A paragraph's words: its runs' text, joined. A picture among them (a run
-- `{ x = alt, img = src }`) is drawn as a picture or as a link, never as
-- words to find: it counts as one byte no query holds (NUL), so ds/markdown
-- marks the same matches whichever it draws, and none spans it.
M.PICTURE = "\0"
function M.words(runs: { MarkdownRun }): string
	local out: { string } = {}
	for _, r in runs do
		table.insert(out, if r.img then M.PICTURE else r.x)
	end
	return table.concat(out)
end

-- A block's parts, in the order they're drawn. (Display math draws a
-- formula, not its source, and a picture its picture: neither has words to
-- find.)
function M.parts(b: MarkdownBlock, out: { string }?): { string }
	local list = out or {}
	if b.t == "p" or b.t == "h" then
		table.insert(list, M.words(b.s))
	elseif b.t == "code" then
		if b.lang ~= "math" then
			table.insert(list, b.text)
		end
	elseif b.t == "ul" or b.t == "ol" then
		for _, item in b.items do
			for _, child in item do
				M.parts(child, list)
			end
		end
	elseif b.t == "quote" then
		for _, child in b.b do
			M.parts(child, list)
		end
	elseif b.t == "table" then
		for c, head in b.head do
			table.insert(list, M.words(head))
			for _, row in b.rows do
				table.insert(list, M.words(row[c] or {}))
			end
		end
	end
	return list
end

-- How many times `q` (lower case) falls in a block.
function M.count(b: MarkdownBlock, q: string): number
	local n = 0
	for _, part in M.parts(b) do
		n += #M.occurrences(part, q)
	end
	return n
end

return M
