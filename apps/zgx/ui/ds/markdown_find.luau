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

-- (By its path: a test reads this module directly.)
local Source = require("./math_source")
local prepare = Source.prepare

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
M.together = together

-- Whether two parsed blocks are the same throughout.
local function alike(a: any, b: any): boolean
	if a == b then
		return true
	end
	if type(a) ~= "table" or type(b) ~= "table" then
		return false
	end
	for k, v in a do
		if not alike(v, b[k]) then
			return false
		end
	end
	for k in b do
		if a[k] == nil then
			return false
		end
	end
	return true
end

-- A text that grows under a key (a reply streaming in), parsed for what's
-- new alone. Its math is read on from where the last read could go on
-- (ds/math_source `feed`). Its settled start (`head`: up to a line after a
-- blank one that nothing later can join, ds/math_source's `cut`) keeps the
-- blocks it was parsed into, and only what follows it (the block being
-- written, about) goes to ui.markdown under the key. (Its math read makes
-- that part's text change in place, not only grow, as a `$x$` closes: so
-- ui.markdown parses again what's past the cut alone, not the whole.) The
-- head's pictures are put together as it grows, a run at its end waiting
-- for what follows.
type Stream = {
	scan: Source.Scan,
	breaks: boolean,
	-- the pieces of the read the head is made of, and its blocks
	cut: number,
	head: { MarkdownBlock },
	-- a cut tried and found to parse otherwise in two (not tried again)
	tried: number,
	-- the head's blocks with pictures together, as far as `grouped`, and the
	-- pictures at its end
	together: { MarkdownBlock },
	carry: { MarkdownBlock },
	grouped: number,
}
local streams: { [string]: Stream } = {}
-- How many of a list's first blocks are the very tables the last parse
-- under its key began with (ds/markdown keeps what it drew of them).
local settled_of: { [any]: number } = setmetatable({}, { __mode = "k" }) :: any
local stream_count = 0

local function stream_of(key: string, breaks: boolean): Stream
	local s = streams[key]
	if s and s.breaks == breaks then
		return s
	end
	if not s then
		-- (A text's while it streams; forgotten in a lump now and then.)
		stream_count += 1
		if stream_count > 64 then
			streams, stream_count = {}, 1
		end
	end
	local made: Stream = {
		scan = Source.scan(),
		breaks = breaks,
		cut = 0,
		head = {},
		tried = 0,
		together = {},
		carry = {},
		grouped = 0,
	}
	streams[key] = made
	return made
end

local function streamed(text: string, o: MarkdownOptions): { MarkdownBlock }
	local s = stream_of(o.key :: string, o.breaks == true)
	local sc = s.scan
	local over = Source.feed(sc, text)
	if over or sc.cut < s.cut then
		s.cut, s.head, s.tried, s.together, s.carry, s.grouped = 0, {}, 0, {}, {}, 0
	end
	-- (What the last parse began with, before the head gains this one's.)
	local settled = #s.together
	-- (A find's current block, which a lazy parse parses first, counted from
	-- the head's end.)
	local first = (o :: any).first
	local opts: MarkdownOptions = o
	if first ~= nil and #s.head > 0 then
		opts = table.clone(o) :: any
		;(opts :: any).first = if first > #s.head then first - #s.head else nil
	end
	local blocks = ui.markdown(Source.since(sc, s.cut), opts)
	-- Cut further on: what's between the cuts joins the head, if parsed in
	-- two it's what it is parsed whole (pending ones, a long text landing
	-- whole and parsed over frames, wait until they're parsed).
	if sc.cut > s.cut and sc.cut ~= s.tried then
		local pending = false
		for _, b in blocks do
			if b.t == "pending" then
				pending = true
				break
			end
		end
		if not pending then
			s.tried = sc.cut
			local rest = ui.markdown(Source.since(sc, sc.cut), { key = o.key, breaks = o.breaks })
			local k = #blocks - #rest
			local fits = k >= 0
			if fits then
				for j = 1, #rest do
					if not alike(blocks[k + j], rest[j]) then
						fits = false
						break
					end
				end
			end
			if fits then
				table.move(blocks, 1, k, #s.head + 1, s.head)
				s.cut = sc.cut
				blocks = rest
			else
				-- (The key's parse goes on from the head's end again.)
				blocks = ui.markdown(Source.since(sc, s.cut), opts)
			end
		end
	end
	-- The head's pictures together, the blocks it gained alone.
	local head, grouped, carry = s.head, s.together, s.carry
	for j = s.grouped + 1, #head do
		local b = head[j]
		if b.t == "img" then
			table.insert(carry, b)
		else
			if #carry > 0 then
				for _, g in together(table.clone(carry)) do
					table.insert(grouped, g)
				end
				table.clear(carry)
			end
			table.insert(grouped, b)
		end
	end
	s.grouped = #head
	if #grouped == 0 and #carry == 0 then
		return together(blocks)
	end
	local joined: { MarkdownBlock } = blocks
	if #carry > 0 then
		joined = table.clone(carry)
		table.move(blocks, 1, #blocks, #carry + 1, joined)
	end
	local tail = together(joined)
	local out = table.create(#grouped + #tail) :: { MarkdownBlock }
	table.move(grouped, 1, #grouped, 1, out)
	table.move(tail, 1, #tail, #grouped + 1, out)
	settled_of[out] = settled
	return out
end

-- How many of `list`'s first blocks (a parse under a key) are the very
-- tables the last parse under that key began with: 0 when unknown.
function M.settled(list: { MarkdownBlock }): number
	return settled_of[list] or 0
end

-- A text's top-level blocks, as ds/markdown draws them (it parses with
-- this): its math read first (ds/math_source), pictures together one block.
-- `opts`: ui.markdown's (`breaks`: a line break is a break, as a prompt's
-- are; `key`: a text that grows, parsed for what's new alone), or `breaks`
-- alone.
function M.parse(text: string, opts: (boolean | MarkdownOptions)?): { MarkdownBlock }
	local o: MarkdownOptions = if type(opts) == "table" then opts else { breaks = opts == true }
	if o.key then
		return streamed(text, o)
	end
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
		-- (Row by row, as ds/markdown builds them.)
		for _, head in b.head do
			table.insert(list, M.words(head))
		end
		for _, row in b.rows do
			for c in b.head do
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
