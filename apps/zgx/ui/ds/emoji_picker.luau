-- An emoji picker: a search over every emoji (by name and keyword, in
-- English and Chinese), what you picked lately, then each kind in turn
-- (smileys, people, nature, food, travel, activities, objects, symbols,
-- flags) in one grid that scrolls, its tabs following the scroll and
-- jumping to a kind. People and hands take the skin tone chosen beside the
-- search. At its foot, the emoji under the pointer (or the one chosen) and
-- its name, large enough to see.
--
--   EmojiPicker { value = chosen, on_pick = function(e) … end,
--     recents = ui.persist("emoji_recents", {}), tone = ui.persist("emoji_tone", 0) }
--
-- Enter in the search picks its first match. The catalog (ds/emoji_data,
-- Unicode's emoji-test with CLDR keywords) is read the first time a picker
-- is made.

local data = require("@self/emoji_data")
local locale = require("@self/locale")
local Icon = require("@self/icon")
local SearchField = require("@self/search_field")
local Rail = require("@self/rail")

export type Props = {
	-- The emoji chosen now (ringed in the grid, shown at the foot).
	value: State<string?>?,
	on_pick: (emoji: string) -> (),
	-- What was picked lately, most recent first (the picker adds to it).
	recents: State<{ string }>?,
	-- The skin tone: 0 none (the yellow), 1 to 5 light to dark.
	tone: State<number>?,
	-- Emoji across a row, and how tall the grid stands.
	columns: number?,
	height: number?,
	autofocus: boolean?,
	id: string?,
}

-- A cell (the emoji's square), the type size of its glyph, a kind's
-- heading, and how many recents are kept (two rows' worth at most).
local CELL = 38
local GLYPH = 24
local HEAD = 28
local RECENTS = 22

local KINDS: { { id: string, icon: Icon.Name } } = {
	{ id = "recent", icon = "clock-circle" },
	{ id = "smileys", icon = "emoji-funny-circle" },
	{ id = "people", icon = "hand-shake" },
	{ id = "nature", icon = "leaf" },
	{ id = "food", icon = "donut-bitten" },
	{ id = "travel", icon = "bus" },
	{ id = "activities", icon = "football" },
	{ id = "objects", icon = "lightbulb-minimalistic" },
	{ id = "symbols", icon = "heart-angle" },
	{ id = "flags", icon = "flag" },
}

-- The skin tones' modifiers (U+1F3FB..U+1F3FF), and the hand that shows
-- the one chosen.
local TONES = { 0x1F3FB, 0x1F3FC, 0x1F3FD, 0x1F3FE, 0x1F3FF }
local HAND = "✋"

type Entry = { e: string, name: string, words: string, tone: boolean, kind: string }

-- The catalog, read once for every picker: each kind's emoji, every one by
-- its emoji (a recent's name), and what a search reads.
local catalog: { kinds: { { id: string, list: { Entry } } }, by: { [string]: Entry }, all: { Entry } }? = nil
local function read(): typeof(catalog :: any)
	if catalog then
		return catalog
	end
	local out = { kinds = {}, by = {}, all = {} }
	for _, k in data.kinds do
		local list: { Entry } = {}
		for line in string.gmatch(k.list, "[^\n]+") do
			local e, name, en, zh, tone = string.match(line, "^([^\t]*)\t([^\t]*)\t([^\t]*)\t([^\t]*)\t?([^\t]*)$")
			if e and e ~= "" then
				local entry: Entry = {
					e = e,
					name = name,
					words = string.lower(name .. " " .. string.gsub(en, "|", " ")) .. " " .. string.gsub(zh, "|", " "),
					tone = tone == "1",
					kind = k.id,
				}
				table.insert(list, entry)
				table.insert(out.all, entry)
				out.by[e] = entry
			end
		end
		table.insert(out.kinds, { id = k.id, list = list })
	end
	catalog = out
	return out
end

-- An emoji in a tone: the modifier after its first code point (a
-- presentation selector there goes: the modifier implies it).
local function toned(entry: Entry, tone: number): string
	if tone < 1 or tone > 5 or not entry.tone then
		return entry.e
	end
	local cps: { number } = {}
	for _, c in utf8.codes(entry.e) do
		table.insert(cps, c)
	end
	local out = { cps[1], TONES[tone] }
	for i = 2, #cps do
		if not (i == 2 and cps[i] == 0xFE0F) then
			table.insert(out, cps[i])
		end
	end
	return utf8.char(table.unpack(out))
end

-- The words a name is shown in: its first letter up ("Grinning face").
local function title(name: string): string
	return (string.gsub(name, "^%l", string.upper))
end

-- The emoji a search finds, best first: a name starting with it, then a
-- word of the name or a keyword starting with it, then any that hold it.
local function search(query: string): { Entry }
	local q = string.lower((string.gsub(query, "^%s*(.-)%s*$", "%1")))
	local first, word, any = {}, {}, {}
	for _, entry in read().all do
		local name = string.lower(entry.name)
		if string.sub(name, 1, #q) == q then
			table.insert(first, entry)
		elseif string.find(" " .. entry.words, " " .. q, 1, true) then
			table.insert(word, entry)
		elseif string.find(entry.words, q, 1, true) then
			table.insert(any, entry)
		end
	end
	for _, e in word do
		table.insert(first, e)
	end
	for _, e in any do
		table.insert(first, e)
	end
	return first
end

type Row = { key: string, head: string?, cells: { Entry }? }

return ui.component(function(props: Props): Node
	local columns = props.columns or 10
	local value: State<string?> = props.value or ui.use_state(nil :: string?)
	local recents: State<{ string }> = props.recents or ui.use_state({} :: { string })
	local tone: State<number> = props.tone or ui.use_state(0)
	local query = ui.use_state("")
	local hovered = ui.use_state(nil :: string?)
	local toning = ui.use_state(false)
	local offset = ui.use_state(0)
	local seen = ui.use_state(props.height or 232)
	-- A cell's side: the grid's width shared by its columns (CELL until it
	-- has one), so a row fills it edge to edge.
	local cell_side = ui.use_state(CELL)

	local function pick(entry: Entry)
		local e = toned(entry, tone:peek())
		local next: { string } = { entry.e }
		for _, r in recents:peek() do
			if r ~= entry.e and #next < RECENTS then
				table.insert(next, r)
			end
		end
		recents:set(next)
		value:set(e)
		props.on_pick(e)
	end

	-- The grid as rows: each kind's heading, then its emoji a row at a
	-- time; a search's matches under one heading. A row's key says what it
	-- holds (and in which tone), so a row whose emoji change is made anew.
	local rows = ui.computed(function(): { Row }
		local out: { Row } = {}
		local t = tone:get()
		local function add(kind: string, head: string, list: { Entry })
			table.insert(out, { key = `head:{kind}`, head = head })
			for i = 1, #list, columns do
				local cells: { Entry } = {}
				for j = i, math.min(i + columns - 1, #list) do
					table.insert(cells, list[j])
				end
				table.insert(out, { key = `{kind}:{i}:{t}:{cells[1].e}:{#cells}:{cell_side:get()}`, cells = cells })
			end
		end
		local q = query:get()
		if string.find(q, "%S") then
			local found = search(q)
			if #found > 0 then
				add(`found:{q}`, "found", found)
			end
			return out
		end
		local cat = read()
		local lately: { Entry } = {}
		for _, e in recents:get() do
			local entry = cat.by[e]
			if entry then
				table.insert(lately, entry)
			end
		end
		if #lately > 0 then
			add("recent:" .. table.concat(recents:get(), ""), "recent", lately)
		end
		for _, k in cat.kinds do
			add(k.id, k.id, k.list)
		end
		return out
	end)
	local heights = ui.computed(function(): { number }
		local out: { number } = {}
		local side = cell_side:get()
		for _, r in rows:get() do
			table.insert(out, if r.head then HEAD else side)
		end
		return out
	end)
	local total = ui.computed(function(): number
		local sum = 0
		for _, h in heights:get() do
			sum += h
		end
		return sum
	end)
	-- Where each kind's heading starts, for the tabs.
	local tops = ui.computed(function(): { [string]: number }
		local out: { [string]: number } = {}
		local y = 0
		local side = cell_side:get()
		for _, r in rows:get() do
			if r.head then
				out[r.head] = y
			end
			y += if r.head then HEAD else side
		end
		return out
	end)
	-- The kind the grid shows now: the last whose heading is at or above
	-- its top.
	local function current(): string?
		local at = offset:get() + 1
		local best, best_y = nil, -1
		for kind, y in tops:get() do
			if y <= at and y > best_y then
				best, best_y = kind, y
			end
		end
		return best
	end

	local function heading(kind: string): Node
		return ui.row {
			h = HEAD,
			w_full = true,
			items = "end",
			pb = "$space.1",
			px = "$space.1",
			ui.text {
				if kind == "found" then locale.t("ds.emoji_picker.results") else locale.t(`ds.emoji_picker.kind.{kind}`),
				text_size = "$font.size.xs",
				weight = "$font.weight.medium",
				color = "$text.tertiary",
			},
		}
	end

	local function cell(entry: Entry): Node
		local shown = toned(entry, tone:peek())
		local side = cell_side:peek()
		return ui.div {
			w = side,
			h = side,
			shrink = 0,
			rounded = "$radius.md",
			items = "center",
			justify = "center",
			cursor = "pointer",
			role = "button",
			label = title(entry.name),
			bg = function(): string
				local v = value:get()
				return if v ~= nil and (v == shown or v == entry.e) then "$zeron.selected.flat.bg" else "#00000000"
			end,
			hover = { bg = "$hover" },
			active = { bg = "$pressed" },
			on_hover = function(inside: boolean)
				if inside then
					hovered:set(entry.e)
				elseif hovered:peek() == entry.e then
					hovered:set(nil)
				end
			end,
			on_click = function()
				pick(entry)
			end,
			ui.text { shown, text_size = ui.raw(GLYPH, "an emoji's glyph, sized to its cell"), line_height = side, aria_hidden = true },
		}
	end

	local by_key: { [string]: Row } = {}
	local grid = ui.div {
		relative = true,
		w_full = true,
		h = props.height or 232,
		on_resize = function(size)
			seen:set(size.h)
			local side = math.max(30, math.floor(size.w / columns))
			if side ~= cell_side:peek() then
				cell_side:set(side)
			end
		end,
		ui.list {
			w_full = true,
			h_full = true,
			offset = offset,
			overscan = 4,
			heights = function(): { number }
				return heights:get()
			end,
			key = function(i: number): string
				local r = rows:get()[i]
				if r then
					by_key[r.key] = r
					return r.key
				end
				return `none:{i}`
			end,
			row = function(key: string): Node
				local r = by_key[key]
				if not r then
					return ui.div {}
				end
				if r.head then
					return heading(r.head)
				end
				local cells: { Node } = {}
				for _, entry in r.cells or {} do
					table.insert(cells, cell(entry))
				end
				return ui.row { w_full = true, h = cell_side:peek(), table.unpack(cells) }
			end,
			row_text = function(i: number): string
				local r = rows:peek()[i]
				if not r or not r.cells then
					return ""
				end
				local names: { string } = {}
				for _, entry in r.cells do
					table.insert(names, entry.name)
				end
				return table.concat(names, ", ")
			end,
		},
		Rail { offset = offset, extent = function(): number
			return total:get()
		end, view = seen, id = if props.id then `{props.id}-rail` else nil },
		-- Nothing found: says so, where the grid was.
		ui.show(function(): boolean
			return #rows:get() == 0
		end, ui.column {
			absolute = true,
			left = 0,
			right = 0,
			top = 0,
			bottom = 0,
			items = "center",
			justify = "center",
			gap = "$space.1",
			ui.text { locale.t("ds.emoji_picker.none"), text_size = "$font.size.md", color = "$text.secondary" },
			ui.text { locale.t("ds.emoji_picker.none_hint"), text_size = "$font.size.sm", color = "$text.tertiary" },
		}),
	}

	-- The tabs: a kind's glyph each, the one the grid shows lit; a click
	-- scrolls to it. (Away while searching: the matches are one list.)
	local tabs: { Node } = {}
	for _, k in KINDS do
		local function lit(): boolean
			return current() == k.id
		end
		table.insert(
			tabs,
			ui.show(function(): boolean
				return k.id ~= "recent" or #recents:get() > 0
			end, ui.button {
				label = locale.t(`ds.emoji_picker.kind.{k.id}`),
				tooltip = locale.s(`ds.emoji_picker.kind.{k.id}`),
				w = 30,
				h = 28,
				px = 0,
				py = 0,
				rounded = "$radius.md",
				justify = "center",
				items = "center",
				cursor = "pointer",
				bg = function(): string
					return if lit() then "$zeron.selected.flat.bg" else "#00000000"
				end,
				color = function(): string
					return if lit() then "$text.primary" else "$text.tertiary"
				end,
				hover = { bg = "$hover", color = "$text.secondary" },
				active = { bg = "$pressed" },
				on_click = function()
					local y = tops:peek()[k.id]
					if y then
						offset:set(math.min(y, math.max(0, total:peek() - seen:peek())))
					end
				end,
				Icon { name = k.icon, size = 16 },
			})
		)
	end

	-- The tone: a hand in it beside the search; pressed, the six to choose
	-- from in the search's place.
	local function hand(t: number): string
		return toned({ e = HAND, name = "", words = "", tone = true, kind = "" }, t)
	end
	local tone_names = { "default", "light", "medium_light", "medium", "medium_dark", "dark" }
	local tone_choices: { Node } = {}
	for t = 0, 5 do
		table.insert(
			tone_choices,
			ui.button {
				label = locale.t(`ds.emoji_picker.tone.{tone_names[t + 1]}`),
				tooltip = locale.s(`ds.emoji_picker.tone.{tone_names[t + 1]}`),
				w = 32,
				h = 32,
				px = 0,
				py = 0,
				rounded = "$radius.md",
				justify = "center",
				items = "center",
				cursor = "pointer",
				bg = function(): string
					return if tone:get() == t then "$zeron.selected.flat.bg" else "#00000000"
				end,
				hover = { bg = "$hover" },
				active = { bg = "$pressed" },
				on_click = function()
					tone:set(t)
					toning:set(false)
				end,
				ui.text { hand(t), text_size = ui.raw(20, "a hand's glyph, sized to its button"), aria_hidden = true },
			}
		)
	end

	local top = ui.switch(function(): string
		return if toning:get() then "tones" else "search"
	end, {
		search = ui.row {
			w_full = true,
			gap = "$space.2",
			items = "center",
			SearchField {
				value = query,
				placeholder = locale.t("ds.emoji_picker.search"),
				name = locale.t("ds.emoji_picker.search"),
				grow = 1,
				autofocus = props.autofocus,
				on_key = function(e: any): boolean?
					if e.key == "Enter" and not (e.shift or e.cmd or e.alt or e.ctrl) then
						local first = rows:peek()[2]
						if first and first.cells and first.cells[1] then
							pick(first.cells[1])
						end
						return true
					end
					return nil
				end,
			},
			ui.button {
				label = locale.t("ds.emoji_picker.tone.title"),
				tooltip = locale.s("ds.emoji_picker.tone.title"),
				w = 32,
				h = 32,
				px = 0,
				py = 0,
				shrink = 0,
				rounded = "$radius.md",
				justify = "center",
				items = "center",
				cursor = "pointer",
				bg = "#00000000",
				hover = { bg = "$hover" },
				active = { bg = "$pressed" },
				on_click = function()
					toning:set(true)
				end,
				ui.text {
					function(): string
						return hand(tone:get())
					end,
					text_size = ui.raw(20, "a hand's glyph, sized to its button"),
					aria_hidden = true,
				},
			},
		},
		tones = ui.row {
			w_full = true,
			h = 32,
			gap = "$space.1",
			items = "center",
			ui.text { locale.t("ds.emoji_picker.tone.title"), text_size = "$font.size.sm", color = "$text.secondary", grow = 1 },
			table.unpack(tone_choices),
		},
	})

	-- The foot: the emoji under the pointer, else the one chosen, and its
	-- name.
	local function featured(): (string?, string?)
		local h = hovered:get()
		local cat = read()
		if h and cat.by[h] then
			return toned(cat.by[h], tone:get()), title(cat.by[h].name)
		end
		local v = value:get()
		if v then
			-- (A toned one's name is its base's.)
			for _, entry in cat.all do
				if entry.e == v or toned(entry, tone:get()) == v then
					return v, title(entry.name)
				end
			end
			return v, nil
		end
		return nil, nil
	end
	local foot = ui.row {
		w_full = true,
		h = 32,
		gap = "$space.2",
		items = "center",
		px = "$space.1",
		ui.text {
			function(): string
				local e = featured()
				return e or ""
			end,
			text_size = ui.raw(22, "the emoji at the picker's foot, larger than the grid's"),
			aria_hidden = true,
		},
		ui.text {
			function(): string
				local e, name = featured()
				return if e then (name or "") else locale.s("ds.emoji_picker.hint")
			end,
			text_size = "$font.size.sm",
			color = function(): string
				local e = featured()
				return if e then "$text.secondary" else "$text.tertiary"
			end,
			truncate = true,
			grow = 1,
		},
	}

	return ui.column {
		id = props.id,
		w_full = true,
		gap = "$space.2",
		top,
		ui.show(function(): boolean
			return not string.find(query:get(), "%S")
		end, ui.row { w_full = true, justify = "between", items = "center", table.unpack(tabs) }),
		grid,
		ui.div { w_full = true, h = 1, bg = "$border" },
		foot,
	}
end)
