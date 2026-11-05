-- A browser annotation: an element (or a region) picked on a page in the
-- panel's browser, carried with a message. It rides the prompt as the
-- shipping app's token, `[Annotation N](zeron-annotation:<hex JSON>)`
-- (crates/proto/src/annotation.rs): the engine swaps each for
-- `[Annotation N]` and appends one text block per element (page, selector,
-- path, box, styles, a fenced HTML excerpt) for every harness, so nothing
-- past the composer has to know the shape. The engine takes a token only
-- when it is byte for byte what it would write itself, so `token` writes
-- the JSON as serde does: the struct's fields in order, every one present,
-- the same limits applied first (`bounded`), the same escapes.
--
--   Annotation.chip(pick, id, extra)      the composer's cargo item for a pick
--   Annotation.token(a)                   its token for the prompt
--   Annotation.with_annotations(text, items)   the prompt's words, the tokens
--                                          after them (a note after its token)
--   Annotation.shown(items)               the chips a sent message shows for them

local M = {}

export type Annotation = {
	index: number,
	url: string,
	title: string,
	-- CSS shorthand ("button#save.btn.primary"), or a region's words.
	element: string,
	-- A selector that matched this element alone when it was picked.
	selector: string,
	-- Ancestors from body, at most eight, each in `element`'s form.
	path: { string },
	role: string,
	-- Accessible name (aria-label, label text, alt, or visible text).
	name: string,
	text: string,
	-- outerHTML with nested children collapsed and long values trimmed.
	html: string,
	-- Viewport CSS pixels: x, y, width, height.
	rect: { number },
	viewport: { number },
	-- Non-default computed styles, { property, value }.
	styles: { { string } },
}

M.SCHEME = "zeron-annotation:"

-- The field limits, the engine's (applied on capture and again on parse).
local MAX_HTML, MAX_TEXT, MAX_SHORT, MAX_URL, MAX_PATH, MAX_STYLES = 2400, 240, 240, 2048, 8, 24

-- `s` cut to at most `max` bytes at a character boundary, an ellipsis after
-- the cut (serde's `clip`).
local function clip(s: string, max: number): string
	if #s <= max then
		return s
	end
	local e = max
	while e > 0 and bit32.band(string.byte(s, e + 1) or 0, 0xC0) == 0x80 do
		e -= 1
	end
	return `{string.sub(s, 1, e)}…`
end

local function str(v: any): string
	return if type(v) == "string" then v else ""
end

local function int(v: any): number
	local n = if type(v) == "number" then v else 0
	return math.floor(n + 0.5)
end

local function ints(v: any, count: number): { number }
	local out = {}
	for i = 1, count do
		out[i] = int(if type(v) == "table" then v[i] else nil)
	end
	return out
end

-- A pick as the page gave it, bounded (whatever shape it came in).
function M.bounded(a: { [string]: any }): Annotation
	local path = {}
	if type(a.path) == "table" then
		for i, step in a.path do
			if i > MAX_PATH then
				break
			end
			table.insert(path, clip(str(step), MAX_SHORT))
		end
	end
	local styles = {}
	if type(a.styles) == "table" then
		for i, pair in a.styles do
			if i > MAX_STYLES then
				break
			end
			if type(pair) == "table" then
				table.insert(styles, { clip(str(pair[1]), 64), clip(str(pair[2]), MAX_SHORT) })
			end
		end
	end
	return {
		index = int(a.index),
		url = clip(str(a.url), MAX_URL),
		title = clip(str(a.title), MAX_SHORT),
		element = clip(str(a.element), MAX_SHORT),
		selector = clip(str(a.selector), MAX_SHORT * 2),
		path = path,
		role = clip(str(a.role), MAX_SHORT),
		name = clip(str(a.name), MAX_SHORT),
		text = clip(str(a.text), MAX_TEXT),
		html = clip(str(a.html), MAX_HTML),
		rect = ints(a.rect, 4),
		viewport = ints(a.viewport, 2),
		styles = styles,
	}
end

-- A JSON string as serde_json writes one: `"` and `\` escaped, control
-- characters as \b \f \n \r \t or \u00xx (lowercase), DEL and everything
-- else as it is.
local function quote(s: string): string
	local out = string.gsub(s, '[%c"\\]', function(c: string): string
		local b = string.byte(c)
		if c == '"' then
			return '\\"'
		elseif c == "\\" then
			return "\\\\"
		elseif b == 8 then
			return "\\b"
		elseif b == 12 then
			return "\\f"
		elseif b == 10 then
			return "\\n"
		elseif b == 13 then
			return "\\r"
		elseif b == 9 then
			return "\\t"
		elseif b == 127 then
			return c
		end
		return string.format("\\u%04x", b)
	end)
	return `"{out}"`
end

local function list(items: { string }): string
	return `[{table.concat(items, ",")}]`
end

local function numbers(ns: { number }): string
	local out = {}
	for _, n in ns do
		table.insert(out, string.format("%d", n))
	end
	return list(out)
end

-- The JSON, in the struct's field order.
function M.json(a: Annotation): string
	local path = {}
	for _, step in a.path do
		table.insert(path, quote(step))
	end
	local styles = {}
	for _, pair in a.styles do
		table.insert(styles, list({ quote(pair[1]), quote(pair[2]) }))
	end
	return table.concat({
		'{"index":',
		string.format("%d", a.index),
		',"url":',
		quote(a.url),
		',"title":',
		quote(a.title),
		',"element":',
		quote(a.element),
		',"selector":',
		quote(a.selector),
		',"path":',
		list(path),
		',"role":',
		quote(a.role),
		',"name":',
		quote(a.name),
		',"text":',
		quote(a.text),
		',"html":',
		quote(a.html),
		',"rect":',
		numbers(a.rect),
		',"viewport":',
		numbers(a.viewport),
		',"styles":',
		list(styles),
		"}",
	})
end

function M.label(index: number): string
	return `Annotation {index}`
end

-- The prompt's token.
function M.token(a: { [string]: any }): string
	local b = M.bounded(a)
	local hex = string.gsub(M.json(b), ".", function(c: string): string
		return string.format("%02x", string.byte(c))
	end)
	return `[{M.label(b.index)}]({M.SCHEME}{hex})`
end

-- What a pick's chip says of it: the start of the element's text; with
-- none, its accessible name; with none, its role (or tag) and a class
-- (`code`: set in the mono face).
local LABEL = 40
function M.label_of(a: Annotation): (string, boolean)
	local text = string.gsub(string.gsub(a.text, "%s+", " "), "^ ", "")
	if text ~= "" then
		if utf8.len(text) and (utf8.len(text) :: number) > LABEL then
			local cut = string.sub(text, 1, (utf8.offset(text, LABEL + 1) :: number) - 1)
			local word = string.match(cut, "^(.*%S)%s+%S*$")
			text = `{word or cut}…`
		end
		return text, false
	end
	if a.name ~= "" then
		return a.name, false
	end
	local tag, class = string.match(a.element, "^([%w%-]+)#?[%w%-_]*(%.?[%w%-_]*)")
	local head = if a.role ~= "" then a.role else (tag or a.element)
	return `{head}{class or ""}`, true
end

-- The composer's cargo item for a pick (ds/composer_cargo's annotation
-- chip): its number and what it says of the element. `extra`: the panel
-- tab it was picked in and the page's marker for it, so the mark follows
-- the chip.
function M.chip(pick: { [string]: any }, id: string, extra: { tab: string?, marker: number? }?): { [string]: any }
	local a = M.bounded(pick)
	local page = if a.title ~= "" then a.title else a.url
	local name, code = M.label_of(a)
	return {
		id = id,
		kind = "annotation",
		name = name,
		code = code,
		meta = page,
		element = a.element,
		page = page,
		box = a.rect,
		html = a.html,
		index = a.index,
		annotation = a,
		tab = if extra then extra.tab else nil,
		marker = if extra then extra.marker else nil,
	}
end

-- The prompt: the words, then each annotation's token on a line of its
-- own, its note (the words typed for it) after a colon. The engine reads
-- the tokens wherever they stand.
function M.with_annotations(text: string, items: { { [string]: any } }): string
	if #items == 0 then
		return text
	end
	local lines = {}
	for _, item in items do
		local a = item.annotation or item
		local line = M.token(a)
		if type(item.note) == "string" and item.note ~= "" then
			line = `{line}: {item.note}`
		end
		table.insert(lines, line)
	end
	local block = table.concat(lines, "\n")
	return if text == "" then block else `{text}\n\n{block}`
end

-- What a sent message shows for them (ds/user_message's chips, as the
-- host's copy shows them: chat.rs `split_annotations`).
function M.shown(items: { { [string]: any } }): { { [string]: any } }
	local out: { { [string]: any } } = {}
	for _, item in items do
		local a = item.annotation or item
		local chip: { [string]: any } = { kind = "file", icon = "cursor", name = M.label(a.index or 0), meta = a.element }
		table.insert(out, chip)
	end
	return out
end

return M
