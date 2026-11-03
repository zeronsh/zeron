-- Settings search: the field atop the list of pages (⌘F), finding pages
-- and the settings on them, and opening one at its row.
--
-- Pages are built by hand, so the index is kept beside them: each page
-- module lists its rows as it loads (`search.index`), each with the words
-- people search by that its title lacks, and wraps each row it lists in
-- `search.anchor(key, row)`, which is how a hit lands on it (the page
-- scrolled to the row, the row lit for a moment). Ranking follows the
-- shipping app's (crates/ui settings/search.rs): every word of the query
-- must match something; a match in a row's title counts most, then its own
-- words, its section, its page; a match at a word's start half again; a
-- title that is the whole query first of all.

local ds = require("@ds")
local route = require("@app/screens/shell/route")

local M = {}

-- A row's words: a string, or a message (`ui.t`), read in the locale
-- shown as the search runs.
export type Words = string | Message
export type Row = { key: string, title: Words, section: Words?, terms: Words? }
export type Hit = {
	-- The page's id, and the row's key (none: the page itself).
	page: string,
	key: string?,
	title: string,
	-- Where it is: its page's title, and its section's.
	where: string,
	score: number,
}

type Entry = { page: string, key: string?, title: Words, page_title: Words, section: Words?, terms: Words }

local TITLE, TERMS, SECTION, PAGE, EXACT = 100, 40, 30, 20, 100

-- The index, page by page (a hot reload lists them again), and a count of
-- its changes, so what's found follows a page coming or going.
M.revision = ui.state("settings_search_revision", 0)
-- (The index itself.)
local pages: { [string]: { Entry } } = {}
local page_order: { string } = {}

-- `rows`, the page's settings, top to bottom; `terms`, what the page is
-- also called.
function M.index(page: string, title: Words, terms: Words, rows: { Row })
	local list: { Entry } = { { page = page, title = title, page_title = title, terms = terms } }
	for _, r in rows do
		table.insert(list, {
			page = page,
			key = r.key,
			title = r.title,
			page_title = title,
			section = r.section,
			terms = r.terms or "",
		})
	end
	if not pages[page] then
		table.insert(page_order, page)
	end
	pages[page] = list
	M.revision:set(M.revision:peek() + 1)
end

-- A page that went (a plugin's settings, the plugin off): out of the index.
function M.unindex(page: string)
	if not pages[page] then
		return
	end
	pages[page] = nil
	local at = table.find(page_order, page)
	if at then
		table.remove(page_order, at)
	end
	M.revision:set(M.revision:peek() + 1)
end

M.query = ui.state("settings_search_query", "")
-- The hit the keys are on, from 1.
M.at = ui.state("settings_search_at", 1)
-- The row a hit landed on, lit for a moment.
M.lit = ui.state("settings_search_lit", nil :: string?)

-- (Words in any script: a byte past ASCII is a letter's, so "语言" is
-- one word, found inside a title as an English word is.)
local function words(s: string): { string }
	local out = {}
	for w in string.gmatch(string.lower(s), "[%w$/&'\128-\255]+") do
		table.insert(out, w)
	end
	return out
end

-- How well one query word matches `field`, at `weight`: none, the weight,
-- or half again where it starts a word.
local function match(word: string, field: string, weight: number): number
	local text = string.lower(field)
	local from = 1
	local best = 0
	while true do
		local at = string.find(text, word, from, true)
		if not at then
			break
		end
		local start = at == 1 or string.find(string.sub(text, at - 1, at - 1), "[%w]") == nil
		best = math.max(best, if start then weight * 3 / 2 else weight)
		from = at + 1
	end
	return best
end

-- Words as the locale shown has them (tracked: what's found follows it).
local function said(w: Words): string
	return if type(w) == "string" then w else (w :: Message):get()
end

function M.find(query: string): { Hit }
	local q = words(query)
	if #q == 0 then
		return {}
	end
	local whole = string.lower((string.gsub(query, "^%s*(.-)%s*$", "%1")))
	local hits: { Hit } = {}
	local n = 0
	for _, page in page_order do
		for _, e in pages[page] or {} do
			n += 1
			local score = 0
			local all = true
			local title, page_title = said(e.title), said(e.page_title)
			local section = if e.section then said(e.section) else nil
			local terms = said(e.terms)
			for _, w in q do
				local s = math.max(
					match(w, title, TITLE),
					match(w, terms, TERMS),
					if section then match(w, section, SECTION) else 0,
					match(w, page_title, PAGE)
				)
				if s == 0 then
					all = false
					break
				end
				score += s
			end
			if all then
				if string.lower(title) == whole then
					score += EXACT
				end
				-- (Ties keep the pages' order, top to bottom.)
				table.insert(hits, {
					page = e.page,
					key = e.key,
					title = title,
					where = if e.key == nil then "" elseif section then `{page_title} · {section}` else page_title,
					score = score * 10000 - n,
				})
			end
		end
	end
	table.sort(hits, function(a: Hit, b: Hit): boolean
		return a.score > b.score
	end)
	return hits
end

-- A hit's identity: its page and row.
local function id_of(h: Hit): string
	return `{h.page}:{h.key or ""}`
end

-- What the query finds now.
M.hits = ui.computed(function(): { Hit }
	M.revision:get()
	return M.find(M.query:get())
end)

-- Opens a hit: its page, scrolled to its row, the row lit. The query
-- stays, so the next hit is a key away.
local lit_timer: (() -> ())? = nil
function M.open(hit: Hit)
	route.settings(hit.page)
	if lit_timer then
		lit_timer()
		lit_timer = nil
	end
	M.lit:set(hit.key)
	if hit.key then
		lit_timer = ui.after(1600, function()
			M.lit:set(nil)
		end)
	end
end

-- Where the settings page scrolls (the shell's, once it says), and the
-- top of the page's view in the window: the shell's title strip (fixed
-- geometry, shell/settings TITLE_H).
local VIEW_TOP = 38
local function scroll_state(): State<number>?
	local shell = require("@app/screens/shell/settings") :: any
	return shell.scroll
end

-- A row a hit can land on: `key` is its index key. While it's the one lit,
-- it washes, and the page scrolls to it (a row's room above it).
-- `fit`: as wide as what it holds (a card standing on its own), not the
-- row's whole width.
function M.anchor(key: string, row: Node, fit: boolean?): Node
	local here = ui.ref()
	ui.effect(function()
		local el = here:get()
		if not el or M.lit:get() ~= key then
			return
		end
		local scroll = scroll_state()
		if scroll and el.y then
			scroll:set(math.max(0, scroll:peek() + el.y - VIEW_TOP - 48))
		end
	end)
	return ui.div {
		ref = here,
		id = `setting-{key}`,
		w_full = not fit,
		relative = true,
		row,
		ui.div {
			absolute = true,
			left = 0,
			top = 0,
			right = 0,
			bottom = 0,
			pointer_events = "none",
			aria_hidden = true,
			bg = "$zeron.selected.flat.bg",
			opacity = function()
				return if M.lit:get() == key then 1 else 0
			end,
		},
	}
end

-- Where a typed word falls in `word`: its first place, from 1, and its
-- length (none: no typed word is in it).
local function found_in(word: string): (number?, number)
	local lower = string.lower(word)
	for _, q in words(M.query:get()) do
		local at = string.find(lower, q, 1, true)
		if at then
			return at, #q
		end
	end
	return nil, 0
end

-- A hit's title, the letters the query matched tinted as the palette tints
-- its matches. A word a run each of before, matched and after, their text
-- following the query, so typing changes the tint in place.
local function titled(title: string): Node
	local runs: { any } = {}
	local first = true
	for word in string.gmatch(title, "%S+") do
		if not first then
			table.insert(runs, " ")
		end
		first = false
		local function part(which: string): () -> string
			return function(): string
				local at, n = found_in(word)
				if not at then
					return if which == "before" then word else ""
				end
				if which == "before" then
					return string.sub(word, 1, at - 1)
				elseif which == "match" then
					return string.sub(word, at, at + n - 1)
				end
				return string.sub(word, at + n)
			end
		end
		table.insert(runs, ui.span { part("before") })
		table.insert(runs, ui.span { part("match"), bg = "$zeron.match.wash", color = "$zeron.match.ink", radius = ui.raw(2, "a match's tint hugs its letters, inside the line") })
		table.insert(runs, ui.span { part("after") })
	end
	local props: { [any]: any } = { text_size = "$font.size.md", line_height = "$font.line.md" }
	for i, r in runs do
		props[i] = r
	end
	return ui.rich(props)
end

-- The field, for ⌘F.
local field_ref = ui.ref()

-- The field atop the list (⌘F), and what it finds under it while
-- something is typed: a row a hit, its page and section under its name,
-- the keys' hit lit; Enter or a click opens it.
function M.Field(): Node
	local function count(): number
		return #M.hits:get()
	end
	local function move(by: number)
		local n = count()
		if n == 0 then
			return
		end
		M.at:set(((M.at:peek() - 1 + by) % n) + 1)
	end
	ui.effect(function()
		M.query:get()
		M.at:set(1)
	end)
	local field = ds.SearchField {
		value = M.query,
		placeholder = ui.t("settings.search.placeholder"),
		shortcut = "cmd+f",
		name = ui.t("settings.search.placeholder"),
		size = "sm",
		w_full = true,
		on_key = function(e: any): boolean?
			if e.key == "ArrowDown" then
				move(1)
				return true
			elseif e.key == "ArrowUp" then
				move(-1)
				return true
			elseif e.key == "Enter" then
				local hit = M.hits:peek()[M.at:peek()]
				if hit then
					M.open(hit)
				end
				return true
			end
			return nil
		end,
	}
	local Icon = require("@ds/icon")
	local function icon_of(page: string): any
		local shell = require("@app/screens/shell/settings") :: any
		local p = shell.page(page)
		return if p and p.icon then p.icon else "settings-minimalistic"
	end
	-- A hit: its page's glyph, its name, and where it is under it (a page
	-- itself, its name alone); the keys' hit on the list's selected wash.
	-- Each hit's place in the list, by its identity: worked out once a
	-- query, so each row looks its own up.
	local places = ui.computed(function(): { [string]: number }
		local out: { [string]: number } = {}
		for i, h in M.hits:get() do
			out[id_of(h)] = i
		end
		return out
	end)
	local function row(hit: State<Hit>): Node
		-- (Its place in the list moves as the query changes: found by key.)
		local mine = id_of(hit:peek())
		local function place(): number?
			return places:get()[mine]
		end
		local function on(): boolean
			return place() == M.at:get()
		end
		return ui.row {
			role = "option",
			label = function()
				return hit:get().title
			end,
			selected = on,
			cursor = "pointer",
			w_full = true,
			min_h = "$zeron.menu.row",
			px = "$space.2",
			py = "$space.1",
			gap = "$space.2",
			items = "center",
			rounded = "$radius.md",
			bg = function()
				return if on() then "$zeron.selected.flat.bg" else "#00000000"
			end,
			color = function()
				return if on() then "$text.primary" else "$text.secondary"
			end,
			hover = { color = "$text.primary" },
			on_click = function()
				M.at:set(place() or 1)
				M.open(hit:peek())
			end,
			ui.div { shrink = 0, y = -0.5, Icon { name = icon_of(hit:peek().page), size = 16 } },
			ui.column {
				grow = 1,
				min_w = 0,
				titled(hit:peek().title),
				ui.show(function()
					return hit:get().where ~= ""
				end, ui.text {
					function()
						return hit:get().where
					end,
					text_size = "$font.size.xs",
					line_height = "$font.line.xs",
					color = "$text.tertiary",
				}),
			},
		}
	end
	local results = ui.column {
		id = "settings-search-results",
		role = "listbox",
		label = ui.t("settings.search.results"),
		w_full = true,
		gap = "$zeron.list.gap",
		ui.each(M.hits, function(_h: Hit, _i: number, hit: State<Hit>): Node
			return row(hit)
		end, {
			key = id_of,
			w_full = true,
			gap = "$zeron.list.gap",
		}),
		ui.show(function()
			return count() == 0
		end, ui.text {
			ui.t("settings.search.empty"),
			px = "$space.2",
			py = "$space.2",
			text_size = "$font.size.sm",
			color = "$text.tertiary",
		}),
	}
	return ui.column {
		id = "settings-search",
		w_full = true,
		gap = "$space.2",
		pb = "$space.2",
		ui.div { ref = field_ref, w_full = true, field },
		ui.show(function()
			return M.query:get() ~= ""
		end, results),
	}
end

-- ⌘F on the settings page: into its search. The shell's "Find" command
-- (⌘F, Edit menu) asks this when nothing focused took the key; binding ⌘F
-- in the menus also keeps zgx's own find in page out of them.
function M.focus(): boolean
	if route.current:peek().name ~= "settings" then
		return false
	end
	local el = field_ref:peek()
	return el ~= nil and el:focus() == true
end

return M
