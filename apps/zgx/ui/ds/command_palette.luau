-- The command palette (⌘K): one field that finds anything, a session, an
-- action, a place, a setting, and does it. A card of the menus' glass over
-- the window, opening with a short scale and fade, its top held still so
-- only its foot moves (easing) as the results grow and shrink.
--
-- What it lists comes in pages: the root, and the pages its rows open
-- ("Switch model ›"), each a chip in the field, Backspace going back. A
-- page may load its items (a branch list from a device): it shows their
-- rows' shapes while it waits. Typing narrows a page by a fuzzy match: the
-- kind of match first (a word's start, then an acronym, a run inside a
-- word, letters scattered through a title), then the field (title over
-- project, branch, device), every word somewhere in the row, the letters it
-- matched marked. With nothing typed, pinned and recently used things come
-- first. At the root, scopes (All, Sessions, Commands, Settings) narrow it
-- to one kind, each saying how many it holds.
--
-- Everything is on the keys, from the field: ↑ ↓ move (round the ends),
-- Enter does the row, ⌘Enter its second action, Tab the next scope, → or
-- Tab opens a row's page, Backspace on an empty field goes back a page (or
-- out of a scope), Escape clears the field, then goes back, then closes;
-- ⌘L types a page's path (a folder's), to edit, and Tab completes what's
-- typed where the page can. A row clicked leaves the keys with the field.
-- The highlight slides from row to row. A row under a moving pointer is the
-- one highlighted; a row scrolled under a resting one is not, so the keys
-- keep their place.
--
-- One grid: every row's glyph sits in the same slot (an icon, a mark, a
-- project's tile, or nothing at all), so every label starts on one line,
-- with the field's text; the section titles sit on the glyphs' edge.
--
-- The list is virtual (`ui.list`, each row's height known, each row keyed
-- by its item): only the rows in view are built, rows follow their items as
-- a search reorders them, and a search narrowing the last one looks only
-- at what that one found, so thousands of sessions cost what dozens do.
--
-- `CommandPalette { open = state, root = { id = "root", title = "Zeron", items = { … } } }`

local motion = require("@self/motion")
local control = require("@self/control")
local Icon = require("@self/icon")
local Mark = require("@self/mark")
local Tile = require("@self/tile")
local Text = require("@self/text")
local FadeText = require("@self/fade_text")
local Switch = require("@self/switch")
local Kbd = require("@self/kbd")
local Menu = require("@self/menu")
local status = require("@self/status")
local ascii = require("@self/ascii")
local material = require("@self/material")
local locale = require("@self/locale")
local EditorMenu = require("@self/editor_menu")

-- A session, as its row in the sidebar's regular density shows it: its
-- agent, its state (or when it last moved), its pull request, its project,
-- branch and device.
export type Session = {
	status: status.Kind?,
	time: (number | string)?,
	agent: Mark.Name?,
	project: string?,
	hue: string?,
	src: string?,
	emoji: string?,
	branch: string?,
	device: string?,
	pr: { number: number, state: ("open" | "merged" | "closed" | "draft")? }?,
	archived: boolean?,
}
-- Its second action (⌘Enter): what the footer calls it, and what it does.
export type Alt = { label: string, run: () -> () }
export type Item = {
	id: string,
	label: string,
	-- The section it lists under, by title ("Actions", "Sessions").
	section: string,
	-- Which scope holds it; by default a session's "sessions", a toggle's
	-- "settings", anything else's "commands".
	scope: string?,
	-- Its glyph: an icon, a harness mark, or a project's tile (its hue or
	-- artwork). A session shows its agent. With none, the slot stays empty.
	icon: Icon.Name?,
	mark: Mark.Name?,
	hue: string?,
	src: string?,
	emoji: string?,
	-- Other words it answers to ("dark" for Appearance), matched as its
	-- label is but weighing less, and never marked.
	keywords: string?,
	-- A muted word at its end (a setting's current value), and the keys
	-- that do it elsewhere ("cmd+n").
	hint: (string | () -> string)?,
	shortcut: string?,
	session: Session?,
	-- A setting, flipped in place (the palette stays open).
	toggle: State<boolean>?,
	-- The page it opens.
	page: (() -> Page)?,
	-- The current choice on its page (a check at its end; highlighted as
	-- the page opens).
	checked: (() -> boolean)?,
	-- Listed first while nothing is typed.
	pinned: boolean?,
	-- What Enter does (the palette closes first), and what the footer calls
	-- it ("Open", "Run"); ⌘Enter's.
	run: (() -> ())?,
	verb: string?,
	alt: Alt?,
}
-- What a page says with nothing in it, or while it loads.
export type Note = { title: string, hint: string? }
export type Page = {
	id: string,
	title: string,
	placeholder: string?,
	items: ({ Item } | () -> { Item })?,
	-- Its items, loaded: `done` is called with them (once). Until then the
	-- page shows its rows' shapes under `loading`'s title.
	load: ((done: ({ Item }) -> ()) -> ())?,
	loading: Note?,
	empty: Note?,
	-- What it finds that it can't list ahead (files in a workspace, a
	-- folder's clone): asked with the search (two letters or more, once
	-- typing pauses), a promise of items, best first (told again as more
	-- comes); and with nothing typed, at once, for what it lists then (its
	-- recent files). The root with one has a Files scope.
	search: ((query: string) -> any)?,
	-- Where it is, as a path to type from (a folder's): ⌘L puts it in the
	-- search, to edit into another (Zeron's address bar).
	path: (() -> string?)?,
	-- What's typed that goes somewhere at once (a folder's `name/`, a path
	-- ending in its separator): the page it opens, the search cleared
	-- behind it.
	typed: ((query: string) -> Page?)?,
	-- Tab: what's typed, completed (a folder's name, its separator after).
	complete: ((query: string) -> string?)?,
	-- A root page's own: the scopes (All, Sessions, Commands, Settings),
	-- Tab and ⌘1–4 moving between them (⌘K's). A root without them is a
	-- step of its own (New project's devices): its title heads the search
	-- as the pages opened from it do, and Tab only completes or opens.
	scopes: boolean?,
}
export type Props = {
	open: State<boolean>,
	root: Page,
	-- What was used lately, by id, most recent first: listed first while
	-- nothing is typed, and a little ahead in a search. Its own by default.
	recent: State<{ string }>?,
}

-- The card's width, and the geometry of what it holds: the search's row,
-- the scopes', the footer's; a command's row, a session's (two lines), a
-- section's title (the first a little shorter: nothing above it to keep
-- from); the inset at the list's ends; a glyph's slot.
local typing_paths: { () -> boolean }
local W = 640
local SEARCH_H, SCOPES_H, FOOT_H = 52, 38, 40
local ROW, SESSION, HEAD, HEAD_FIRST, INSET, SLOT = 36, 52, 32, 26, 6, 18
-- The list's height at most, and an empty one's.
local VIEW_MAX, NOTE_H = 440, 196
-- How far the card's top sits from the window's (clear of its title bar,
-- a little above centre in a window of Zeron's size).
local TOP = 88
-- Kept clear under the card in a short window, and the least the list
-- shrinks to there (about four rows) before the window clips the card.
local BOTTOM_CLEAR, VIEW_MIN = 24, 160

-- The list's height at most in this window: VIEW_MAX, less whatever the
-- card would otherwise run past the window's foot. A host that feeds no
-- window size (a headless test) gets VIEW_MAX.
local function view_max(): number
	local window = (ui :: any).window
	local state = if type(window) == "table" or type(window) == "userdata" then window.size else nil
	local ok, v = pcall(function(): any
		return if state ~= nil then state:get() else nil
	end)
	local h = if ok and type(v) == "table" and type(v.height) == "number" then v.height else 0
	if h <= 1 then
		return VIEW_MAX
	end
	local room = h - TOP - BOTTOM_CLEAR - SEARCH_H - SCOPES_H - FOOT_H
	return math.max(VIEW_MIN, math.min(VIEW_MAX, room))
end
-- With nothing typed, how many sessions All shows before "Show all"; and
-- how many recent things.
local ALL_SESSIONS, RECENT = 6, 5
-- How things move: the card opening, the list's height, the highlight
-- sliding between rows.
local OPEN = { duration = 180, ease = "out(4)" }
local GROW: TweenOptions = { duration = 140, ease = "out(3)" }
local SLIDE_MS, SLIDE_EASE = 140, "outQuint"

-- Between rows: the theme's list gap (read as the list lays out: the
-- themes load after this module).
local function gap(): number
	return (ui.theme.get("zeron.list.gap") :: number?) or 2
end

-- The scopes, in Tab's order (each name its message's key).
local SCOPES = {
	{ id = "all", label = "ds.command_palette.scope.all" },
	{ id = "sessions", label = "ds.command_palette.scope.sessions" },
	{ id = "commands", label = "ds.command_palette.scope.commands" },
	{ id = "settings", label = "ds.command_palette.scope.settings" },
}
-- With a search (Zeron's palette: All, Threads, Commands, Files, ⌘1–4),
-- Files in its place, settings after.
local SCOPES_SEARCHED = {
	{ id = "all", label = "ds.command_palette.scope.all" },
	{ id = "sessions", label = "ds.command_palette.scope.sessions" },
	{ id = "commands", label = "ds.command_palette.scope.commands" },
	{ id = "files", label = "ds.command_palette.scope.files" },
	{ id = "settings", label = "ds.command_palette.scope.settings" },
}

-- The sections the palette names itself (the rows keep these names, as
-- data), each drawn in the locale's words; the rest are their callers'.
local OWN_TITLES: { [string]: string } = {
	Pinned = "ds.command_palette.pinned",
	Recent = "ds.command_palette.recent",
	["Loading…"] = "ds.command_palette.loading",
}
local function title_text(title: string): string
	local key = OWN_TITLES[title]
	return if key then locale.s(key) else title
end
-- A search's wait once typing pauses, the letters it needs, and how many of
-- its finds All shows before "Show all" (its own scope, more).
local SEARCH_WAIT, SEARCH_MIN, SEARCH_ALL, SEARCH_MOST = 80, 2, 5, 150

local function scope_of(item: Item): string
	if item.scope then
		return item.scope
	end
	if item.session then
		return "sessions"
	end
	return if item.toggle then "settings" else "commands"
end

-- A count as it reads: 3,012.
local function count(n: number): string
	local s = tostring(math.floor(n))
	while true do
		local next, k = string.gsub(s, "^(%d+)(%d%d%d)", "%1,%2")
		s = next
		if k == 0 then
			return s
		end
	end
end

-- When a session last moved, for ties (a time as text sorts as now).
local function moved(item: Item): number
	local t = if item.session then item.session.time else nil
	return if type(t) == "number" then t else 0
end

-- Matching. A field's words start after anything that isn't a letter or a
-- digit (bytes past ASCII are letters: they belong to a UTF-8 character).
local function is_start(s: string, i: number): boolean
	if i == 1 then
		return true
	end
	local b = string.byte(s, i - 1) :: number
	if b >= 128 then
		return false
	end
	return not ((b >= 48 and b <= 57) or (b >= 97 and b <= 122))
end

-- Each letter of `term` at the next word's start that has it, in order;
-- nil if some letter starts no word after the last.
local function acronym(field: string, term: string): { number }?
	local out = table.create(#term, 0)
	local from = 1
	for k = 1, #term do
		local c = string.sub(term, k, k)
		local found: number? = nil
		local p = from
		while true do
			local i = string.find(field, c, p, true)
			if not i then
				break
			end
			if is_start(field, i) then
				found = i
				break
			end
			p = i + 1
		end
		if not found then
			return nil
		end
		out[k] = found
		from = found + 1
	end
	return out
end

-- How well `term` matches `field` (lowercased both), as a multiple of the
-- field's weight, and the bytes it matched: a run of the letters starting a
-- word (1.5, and a little more starting the field); every letter a word's
-- start, as an acronym ("nc", New chat: 1.3); a run inside a word (up to 1,
-- less for a short one: "nc" inside "branch" is chance); then the letters
-- in order, scattered (at most 0.8: words' starts and runs count, a
-- spread-out match is none). One letter matches only a word's start.
local function match(field: string, term: string): (number, { number }?)
	local n = #term
	local at, best = 1, 0
	local where: number? = nil
	while true do
		local i = string.find(field, term, at, true)
		if not i then
			break
		end
		if is_start(field, i) then
			best = if i == 1 then 1.6 else 1.5
			where = i
			break
		elseif best == 0 and n > 1 then
			best, where = 0.7 + math.min(n, 5) * 0.06, i
		end
		at = i + 1
	end
	if n > 1 and best < 1.3 then
		local a = acronym(field, term)
		if a then
			return 1.3, a
		end
	end
	if where then
		local out = table.create(n, 0)
		for k = 0, n - 1 do
			out[k + 1] = where + k
		end
		return best, out
	end
	if n < 2 then
		return 0, nil
	end
	-- Scattered: plainly in order, each letter at the next place it is.
	local out = table.create(n, 0)
	local from = 1
	for k = 1, n do
		local i = string.find(field, string.sub(term, k, k), from, true)
		if not i then
			return 0, nil
		end
		out[k] = i
		from = i + 1
	end
	-- Spread out: noise, not a match.
	if out[n] - out[1] + 1 > n * 4 then
		return 0, nil
	end
	local starts, runs = 0, 0
	for k, i in out do
		if is_start(field, i) then
			starts += 1
		elseif k > 1 and out[k - 1] == i - 1 then
			runs += 1
		end
	end
	return 0.3 + 0.5 * math.min(1, (starts + runs * 0.6) / n), out
end

-- A match's kind, as a score that outranks any field's weight: a run at a
-- word's start, an acronym, a run inside a word, letters scattered.
local function class(m: number): number
	if m >= 1.5 then
		return 600
	elseif m >= 1.3 then
		return 400
	elseif m > 0.8 then
		return 200
	end
	return 0
end

-- What a row is searched by: each field lowercased and its weight (a title
-- outweighs the rest put together); all of them in one string, the title
-- alone, and the letters starting their words, for a quick look before
-- matching.
type Field = { key: string, text: string, weight: number }
type Prepared = { fields: { Field }, all: string, label: string, initials: string }
local prepared: { [Item]: Prepared } = setmetatable({}, { __mode = "k" }) :: any
local function prepare(item: Item): Prepared
	local p = prepared[item]
	if p then
		return p
	end
	local label = string.lower(item.label)
	local f = { { key = "label", text = label, weight = 100 } }
	local s = item.session
	if s then
		if s.project then
			table.insert(f, { key = "project", text = string.lower(s.project), weight = 40 })
		end
		if s.branch then
			table.insert(f, { key = "branch", text = string.lower(s.branch), weight = 30 })
		end
		if s.device then
			table.insert(f, { key = "device", text = string.lower(s.device), weight = 20 })
		end
	end
	if item.keywords then
		table.insert(f, { key = "keywords", text = string.lower(item.keywords), weight = 50 })
	end
	-- Its section's title too ("settings" finds every setting), but not a
	-- session's: every session would answer to "s".
	if not s then
		table.insert(f, { key = "section", text = string.lower(item.section), weight = 12 })
	end
	local all = {}
	for _, field in f do
		table.insert(all, field.text)
	end
	local joined = table.concat(all, "\n")
	local initials = {}
	for word in string.gmatch(joined, "%w+") do
		table.insert(initials, string.sub(word, 1, 1))
	end
	p = { fields = f, all = joined, label = label, initials = table.concat(initials) }
	prepared[item] = p
	return p
end

-- A word of the query, with the pattern finding its letters in order (for
-- the quick look: a word neither in the row nor scattered through its title
-- can't match).
type Term = { text: string, scattered: string, starts: string? }
local function terms_of(q: string): { Term }
	local out: { Term } = {}
	for word in string.gmatch(string.lower(q), "%S+") do
		local parts = {}
		for k = 1, #word do
			local c = string.sub(word, k, k)
			table.insert(parts, if string.find(c, "%p") then "%" .. c else c)
		end
		-- (One letter matches only a word's start: looked for among the
		-- words' first letters.)
		local starts = if #word == 1 then word else nil
		table.insert(out, { text = word, scattered = table.concat(parts, ".-"), starts = starts })
	end
	return out
end

-- An item's score for the query's words (nil: some word matches nothing),
-- and the bytes matched in each field.
local function score(item: Item, terms: { Term }): (number?, { [string]: { number } }?)
	local p = prepare(item)
	for _, term in terms do
		if term.starts then
			if not string.find(p.initials, term.starts, 1, true) then
				return nil, nil
			end
		elseif not string.find(p.all, term.text, 1, true) and not string.find(p.label, term.scattered) then
			return nil, nil
		end
	end
	local total = 0
	local hits: { [string]: { number } } = {}
	for _, term in terms do
		local best, best_key, best_at = 0, nil, nil
		for _, field in p.fields do
			local m, at = match(field.text, term.text)
			-- Letters scattered, or two inside a word, count in the title
			-- alone: elsewhere (a branch, keywords) they are chance.
			if field.key ~= "label" and (m <= 0.8 or (m < 1.3 and #term.text < 3)) then
				m = 0
			end
			-- The kind of match first (a run at a word's start in any field
			-- beats letters scattered through the title), then the field.
			local v = if m > 0 then class(m) + m * field.weight else 0
			if v > best then
				best, best_key, best_at = v, field.key, at
			end
		end
		if best == 0 then
			return nil, nil
		end
		total += best
		if best_key and best_at then
			local list = hits[best_key] or {}
			for _, i in best_at do
				table.insert(list, i)
			end
			hits[best_key] = list
		end
	end
	return total, hits
end

-- A page's items: where each stands in it, and all of them most recently
-- moved first (the rest in its order), each worked out once a list (kept across
-- openings: the list is the caller's).
local orders: { [any]: { [Item]: number } } = setmetatable({}, { __mode = "k" }) :: any
local recency: { [any]: { Item } } = setmetatable({}, { __mode = "k" }) :: any
local function order_of(items: { Item }): { [Item]: number }
	local o = orders[items]
	if not o then
		o = {}
		for i, item in items do
			o[item] = i
		end
		orders[items] = o
	end
	return o
end
local function by_recency(items: { Item }): { Item }
	local r = recency[items]
	if not r then
		local o = order_of(items)
		r = table.clone(items)
		table.sort(r, function(a: Item, b: Item): boolean
			local ta, tb = moved(a), moved(b)
			if ta ~= tb then
				return ta > tb
			end
			return o[a] < o[b]
		end)
		recency[items] = r
	end
	return r
end

-- How a marked text is set: its size and ink (tokens), and its box's give.
type TextStyle = { text_size: string, color: string, grow: number?, shrink: number?, weight: string? }

-- A text with the bytes at `at` marked: runs of them a weight up, on a
-- faint wash of the accent. Fading at its end when it doesn't fit, as
-- FadeText does.
local function marked(text: string, at: { number }?, style: TextStyle): Node
	if not at or #at == 0 then
		return FadeText {
			text,
			literal = true,
			text_size = style.text_size,
			color = style.color,
			weight = style.weight,
			grow = style.grow,
			shrink = style.shrink,
		}
	end
	local on: { [number]: boolean } = {}
	for _, i in at do
		on[i] = true
	end
	local clip, inner = ui.ref(), ui.ref()
	local rich: RichProps = {
		ref = inner,
		shrink = 0,
		text_size = style.text_size,
		color = style.color,
		weight = style.weight,
	}
	local i, n = 1, #text
	while i <= n do
		local lit = on[i] == true
		local j = i
		while j < n and (on[j + 1] == true) == lit do
			j += 1
		end
		local piece = string.sub(text, i, j)
		if lit then
			table.insert(rich, ui.span {
				piece,
				literal = true,
				color = "$zeron.match.ink",
				-- (A theme token, which a span takes: zgx's definitions allow
				-- a span's weight numbers only.)
				weight = "$font.weight.semibold" :: any,
				bg = "$zeron.match.wash",
				radius = 3,
				padding = { 0, 1 },
			})
		else
			table.insert(rich, ui.span { piece, literal = true })
		end
		i = j + 1
	end
	return ui.row {
		ref = clip,
		grow = style.grow,
		shrink = style.shrink,
		min_w = 0,
		overflow = "hidden",
		items = "center",
		fade_x = function()
			local over = (inner.width or 0) > (clip.width or 0) + 0.5
			return { 0, if over then 24 else 0 }
		end,
		ui.rich(rich),
	}
end

-- A row of the list as laid out: padding, a section's title, an item, a
-- loading row's shape, or "Show all" (into a scope). `key` is its identity:
-- an item's row follows it as searches reorder the list.
type Row = {
	kind: "pad" | "head" | "item" | "more" | "ghost",
	key: string,
	h: number,
	title: string?,
	count: number?,
	item: Item?,
	scope: string?,
}

-- A scope's chip: chosen, the menus' selected wash, kept under the pointer
-- and the press (a hover wash in its place would dim it); otherwise a wash
-- only as it is touched.
local scope_chip = ui.variants {
	variants = {
		chosen = {
			[true] = {
				bg = "$zeron.menu.selected",
				color = "$text.primary",
				hover = { bg = "$zeron.menu.selected" },
				active = { bg = "$zeron.menu.selected" },
			},
			[false] = {
				bg = "#00000000",
				color = "$text.secondary",
				hover = { bg = "$hover", color = "$text.primary" },
				active = { bg = "$pressed", color = "$text.primary" },
			},
		},
	},
	defaults = { chosen = false },
}

-- A rule across the whole card, edge to edge: a menu's.
local rule = Menu.Rule

-- A pull request's badge, as the sidebar's: its glyph and number in its
-- state's colour, on a wash of it.
local function pr_badge(pr: { number: number, state: ("open" | "merged" | "closed" | "draft")? }): Node
	local state = pr.state or "open"
	local hue = `$zeron.pr.{state}`
	return ui.row {
		shrink = 0,
		relative = true,
		h = 16,
		px = "$space.1",
		gap = "$zeron.space.hair",
		items = "center",
		rounded = "$radius.sm",
		label = locale.t("ds.command_palette.pr", { n = tostring(pr.number), state = state }),
		ui.div { absolute = true, left = 0, right = 0, top = 0, bottom = 0, rounded = "$radius.sm", bg = hue, opacity = 0.1 },
		ui.div { color = hue, opacity = 0.85, Icon { name = if state == "draft" then "pull-request-draft" else "pull-request", size = 11 } },
		ui.text { tostring(pr.number), text_size = "$font.size.xs", weight = "$font.weight.medium", color = hue, tabular = true, opacity = 0.85 },
	}
end

-- What is open: a page, and what it showed when another opened over it
-- (its search, highlight and scope, given back on the way back).
type Level = { page: Page, query: string, hi: string?, scope: string }

local Body = ui.component(function(props: Props & { recent_state: State<{ string }> }): Node
	local open = props.open
	local recent = props.recent_state
	local query = ui.use_state("")
	local scope = ui.use_state("all")
	-- The pages open, the root first (plain: a page holds functions), and a
	-- count that changes as one opens or closes, for what follows them.
	local stack: { Level } = { { page = props.root, query = "", hi = nil, scope = "all" } }
	-- Whether the root has scopes (⌘K's); without, every step is a page.
	local scoped = props.root.scopes == true
	local depth = ui.use_state(1)
	local hi = ui.use_state(nil :: string?)
	local offset = ui.use_state(0)
	-- Pages that load: what each loaded (by page), and whether the one shown
	-- is still loading.
	local loaded: { [Page]: { Item } } = setmetatable({}, { __mode = "k" }) :: any
	local loading = ui.use_state(false)
	local field: any = nil
	local list_ref = ui.ref()

	local function page(): Page
		return stack[#stack].page
	end

	local function items_of(p: Page): { Item }
		if p.load then
			return loaded[p] or {}
		end
		local items = p.items
		if type(items) == "function" then
			return items()
		end
		return items or {}
	end

	-- The list as laid out, beside the state that says it changed (`rev`):
	-- the rows, their heights, by key, the keys' stops, where each row is
	-- and its section's title; what each item matched; counts by scope.
	local rev = ui.use_state(0)
	local rows: { Row } = {}
	local heights: { number } = {}
	local by_key: { [string]: Row } = {}
	local nav: { string } = {}
	local index_of: { [string]: number } = {}
	local head_of: { [string]: number } = {}
	local hits_of: { [string]: { [string]: { number } } } = {}
	local counts = ui.use_state({ all = 0, sessions = 0, commands = 0, settings = 0, files = 0 })
	-- The root's scopes: Files among them where it searches.
	local SCOPES = if props.root.search then SCOPES_SEARCHED else SCOPES
	-- What the root's search found, for which query, and whether it's asking.
	-- (Its rows held aside, as they carry what they do: a state holds data
	-- alone. `found_v` moves as they change.)
	local found_far: { q: string, items: { Item }, page: Page? } = { q = "", items = {} }
	local found_v = ui.use_state(0)
	local function found(q2: string, items: { Item }, on: Page?)
		found_far = { q = q2, items = items, page = on }
		found_v:set(found_v:peek() + 1)
	end
	local searching = ui.use_state(false)
	local search_cancel: (() -> ())? = nil
	ui.effect(function()
		local q = query:get()
		depth:get()
		local on = page()
		if search_cancel then
			search_cancel()
			search_cancel = nil
		end
		-- (The page shown's: the root's files, a page's own finds.)
		local ask = on.search
		local wanted = string.match(q, "^%s*(.-)%s*$") or ""
		-- (Nothing typed, it's asked at once: what it lists ahead, its recent
		-- files. A letter alone, nothing.)
		if not ask or (wanted ~= "" and #wanted < SEARCH_MIN) then
			searching:set(false)
			if #found_far.items > 0 then
				found("", {})
			end
			return
		end
		searching:set(wanted ~= "")
		search_cancel = ui.after(if wanted == "" then 0 else SEARCH_WAIT, function()
			search_cancel = nil
			local asked = ask(wanted)
			if not asked then
				searching:set(false)
				return
			end
			-- (It may answer more than once, as more is found: each answer
			-- replaces the last.)
			asked:after(function(items: any)
				-- (Only for what's still typed.)
				if query:peek() ~= q or page() ~= on then
					return
				end
				found(q, if type(items) == "table" then items else {}, on)
				searching:set(false)
			end, function()
				if query:peek() == q then
					searching:set(false)
				end
			end)
		end)
	end)
	local total_h = ui.use_state(0)
	local shown_rows = ui.use_state(0)
	-- The last search, to narrow from: its page, query and what it found
	-- (before scopes).
	local last: { page: Page?, q: string, found: { Item }, list: { Item }? } = { page = nil, q = "", found = {}, list = nil }
	-- Where the highlight lands as the list changes: on the first row (a new
	-- search, a new page), or kept where it is (anything else).
	local last_query, last_depth, last_scope = nil :: string?, 0, ""
	-- Set as a page opens or closes: the row to highlight on it.
	local land: string? = nil

	local function build()
		local q = query:get()
		local sc = scope:get()
		local d = depth:get()
		local waiting = loading:get()
		local p = page()
		local at_root = d == 1
		local terms = terms_of(q)
		local used = recent:peek()
		local used_at: { [string]: number } = {}
		for i, id in used do
			used_at[id] = i
		end

		-- What to look through: the page's items (with nothing typed, most
		-- recently moved first: sorted once a page), or, narrowing the last
		-- search (the same page, the query grown), what that one found.
		local items = items_of(p)
		local pool = items
		local placed = order_of(items)
		if #terms == 0 then
			pool = by_recency(items)
		elseif last.page == p and last.list == items and last.q ~= "" and string.sub(q, 1, #last.q) == last.q then
			pool = last.found
		end

		-- (`rank`: the score, then when it last moved, as one number to sort
		-- by: scores stay below 10^5 and times below 10^10, well inside a
		-- double's exact range.)
		type Hit = { item: Item, score: number, order: number?, rank: number }
		local n_all, n_kind = 0, { sessions = 0, commands = 0, settings = 0 } :: { [string]: number }
		local found: { Hit } = {}
		local matched: { Item } = {}
		local new_hits: { [string]: { [string]: { number } } } = {}
		for _, item in pool do
			local order = placed[item]
			local archived = item.session ~= nil and item.session.archived == true
			local s = 0
			if #terms == 0 then
				-- Nothing typed: archived sessions only come by search.
				if archived then
					continue
				end
			else
				local got, hits = score(item, terms)
				if not got then
					continue
				end
				s = got
				new_hits[item.id] = hits :: any
				table.insert(matched, item)
				-- A little ahead: what was used lately, what is pinned; behind,
				-- what is archived. Among commands, shorter titles edge ahead
				-- (sessions tie by when they last moved, below).
				local u = used_at[item.id]
				if u then
					s += (RECENT * 2 + 2 - u) * 4
				end
				if item.pinned then
					s += 8
				end
				if archived then
					s *= 0.5
				end
				if not item.session then
					s -= #item.label * 0.05
				end
			end
			local kind = scope_of(item)
			n_all += 1
			n_kind[kind] = (n_kind[kind] or 0) + 1
			if at_root and sc ~= "all" and kind ~= sc then
				continue
			end
			table.insert(found, { item = item, score = s, order = order, rank = math.floor(s * 100) * 1e10 + moved(item) })
		end
		-- What the root's search found for this query, in its order (its
		-- best first: the host ranks them), under strong matches of the rest.
		found_v:get()
		local far = found_far
		if far.page == p and far.q == q then
			for i, item in far.items do
				if i > SEARCH_MOST then
					break
				end
				local kind = scope_of(item)
				n_all += 1
				n_kind[kind] = (n_kind[kind] or 0) + 1
				if sc == "all" or kind == sc then
					local fs = 150 - i * 0.01
					-- (Nothing typed, its list heads the page, as Zeron's recent
					-- files do; searched, it follows the rest.)
					local order = if #terms == 0 then -1e6 + i else 1e6 + i
					table.insert(found, { item = item, score = fs, order = order, rank = math.floor(fs * 100) * 1e10 })
				end
			end
		end
		last.page, last.q, last.found, last.list = p, q, if #terms > 0 then matched else {}, items
		hits_of = new_hits

		-- Into sections: with nothing typed, pinned and recent first, then
		-- each section as the page orders it (sessions by when they last
		-- moved); with a search, every section best match first, the
		-- sections by their best.
		type Section = { title: string, hits: { Hit }, best: number, first: number, more: string?, total: number }
		local sections: { Section } = {}
		local named: { [string]: Section } = {}
		local function section(title: string, first: number): Section
			local s = named[title]
			if s then
				return s
			end
			local new: Section = { title = title, hits = {}, best = -math.huge, first = first, total = 0 }
			named[title] = new
			table.insert(sections, new)
			return new
		end
		if #terms == 0 then
			local taken: { [Item]: boolean } = {}
			if at_root then
				local pinned = section("Pinned", 0)
				for _, h in found do
					if h.item.pinned then
						table.insert(pinned.hits, h)
						taken[h.item] = true
					end
				end
				if sc == "all" then
					local recent_s = section("Recent", 0)
					local by: { [string]: Hit } = {}
					for _, h in found do
						by[h.item.id] = h
					end
					for _, id in used do
						local h = by[id]
						if h and not taken[h.item] and #recent_s.hits < RECENT then
							table.insert(recent_s.hits, h)
							taken[h.item] = true
						end
					end
				end
			end
			for _, h in found do
				if not taken[h.item] then
					table.insert(section(h.item.section, h.order or 1e9).hits, h)
				end
			end
			-- (The pool came most recent first: the sections take the page's
			-- order back.)
			table.sort(sections, function(a: Section, b: Section): boolean
				return a.first < b.first
			end)
		else
			for _, h in found do
				local s = section(h.item.section, h.order or 1e9)
				table.insert(s.hits, h)
				s.best = math.max(s.best, h.score)
			end
			for _, s in sections do
				table.sort(s.hits, function(a: Hit, b: Hit): boolean
					if a.rank ~= b.rank then
						return a.rank > b.rank
					end
					-- (Total: a hit without an order sorts after the ordered ones,
					-- then by score and title.)
					local ao, bo = a.order, b.order
					if ao ~= bo then
						if ao == nil then
							return false
						elseif bo == nil then
							return true
						end
						return ao < bo
					end
					if a.score ~= b.score then
						return a.score > b.score
					end
					return a.item.label < b.item.label
				end)
			end
			table.sort(sections, function(a: Section, b: Section): boolean
				if a.best ~= b.best then
					return a.best > b.best
				end
				return a.first < b.first
			end)
		end
		for _, s in sections do
			s.total = #s.hits
		end
		-- Under All, sessions stop after a few: the rest are a row away
		-- (their scope), so actions and settings stay in reach.
		if at_root and scoped and sc == "all" then
			for _, s in sections do
				local of_sessions = #s.hits > 0 and scope_of(s.hits[1].item) == "sessions"
				if of_sessions and s.title ~= "Pinned" and s.title ~= "Recent" and #s.hits > ALL_SESSIONS + 1 then
					s.hits = table.move(s.hits, 1, ALL_SESSIONS, 1, {})
					s.more = "sessions"
				end
				-- (Files found: a few, the rest in their scope.)
				local of_files = #s.hits > 0 and scope_of(s.hits[1].item) == "files"
				if of_files and #s.hits > SEARCH_ALL + 1 then
					s.hits = table.move(s.hits, 1, SEARCH_ALL, 1, {})
					s.more = "files"
				end
			end
		end

		-- Laid out: the rows, their heights and keys; and for each row the
		-- keys stop at, where it is and where its section's title is.
		local g = gap()
		local new_rows: { Row } = { { kind = "pad", key = "pad:top", h = INSET } }
		local function add_row(row: Row)
			table.insert(new_rows, row)
		end
		-- (Every page's sections say what they are, with their counts: a
		-- step's one section too.)
		local titled = #sections > 0
		if waiting then
			-- Loading: the rows' shapes, under what is loading.
			add_row({ kind = "head", key = "head:loading", h = HEAD_FIRST, title = (p.loading and p.loading.title) or "Loading…" })
			for i = 1, 5 do
				add_row({ kind = "ghost", key = `ghost:{i}`, h = ROW + g, count = i })
			end
		end
		for _, s in sections do
			if waiting or #s.hits == 0 then
				continue
			end
			if titled then
				add_row({
					kind = "head",
					key = "head:" .. s.title,
					h = if #new_rows == 1 then HEAD_FIRST else HEAD,
					title = s.title,
					count = s.total,
				})
			end
			for _, h in s.hits do
				add_row({
					kind = "item",
					key = "item:" .. h.item.id,
					h = (if h.item.session then SESSION else ROW) + g,
					item = h.item,
				})
			end
			if s.more then
				add_row({ kind = "more", key = "more:" .. s.title, h = ROW + g, count = s.total, scope = s.more, title = s.title })
			end
		end
		add_row({ kind = "pad", key = "pad:end", h = INSET })
		if #new_rows == 2 then
			new_rows = {}
		end
		local new_heights = table.create(#new_rows, 0)
		local new_nav: { string } = {}
		local new_by_key: { [string]: Row } = {}
		local new_index: { [string]: number } = {}
		local new_head: { [string]: number } = {}
		local y, head_at = 0, nil :: number?
		for i, r in new_rows do
			new_heights[i] = r.h
			new_by_key[r.key] = r
			new_index[r.key] = i
			if r.kind == "head" then
				head_at = i
			elseif r.kind == "item" or r.kind == "more" then
				table.insert(new_nav, r.key)
				if head_at then
					new_head[r.key] = head_at
				end
				head_at = nil
			end
			y += r.h
		end
		rows, heights, by_key, nav, index_of, head_of = new_rows, new_heights, new_by_key, new_nav, new_index, new_head

		-- The highlight: on the first row for a new search, scope or page (or
		-- where the page asks it to land); otherwise kept on its row, or the
		-- first one left.
		local fresh = q ~= last_query or d ~= last_depth or sc ~= last_scope
		last_query, last_depth, last_scope = q, d, sc
		local now = hi:peek()
		if fresh or now == nil or new_by_key[now] == nil then
			local want = land
			if want and new_by_key[want] then
				land = nil
				hi:set(want)
			else
				if not waiting then
					land = nil
				end
				hi:set(new_nav[1])
				offset:set(0)
			end
		end
		ui.batch(function()
			counts:set({
				all = n_all,
				sessions = n_kind.sessions or 0,
				commands = n_kind.commands or 0,
				settings = n_kind.settings or 0,
				files = n_kind.files or 0,
			})
			total_h:set(y)
			shown_rows:set(#new_nav)
			rev:set(rev:peek() + 1)
		end)
	end
	ui.effect(build)
	-- Opening, the root's items get ready for searching a few hundred a
	-- frame, so the first letter typed finds them ready.
	do
		local items = items_of(props.root)
		local at = 1
		local function warm()
			local stop = math.min(at + 399, #items)
			for i = at, stop do
				prepare(items[i])
			end
			at = stop + 1
			if at <= #items then
				ui.after(16, warm)
			end
		end
		ui.after(16, warm)
	end

	-- The list's height: its rows up to the view (a short list leaves no
	-- room under it), or an empty one's message.
	local function list_h(): number
		if shown_rows:get() == 0 and not loading:get() then
			return NOTE_H
		end
		return math.min(view_max(), total_h:get())
	end
	-- Bring the row `key` into view, with the row past it the way the keys
	-- are moving (`by`), so what comes next always shows clear of the edge's
	-- fade: its section's title with a section's first row; the first row,
	-- the top.
	local function reveal(key: string, by: number?)
		local list = list_ref:peek()
		local i = index_of[key]
		if not list or not i then
			return
		end
		if key == nav[1] then
			list:scroll_to_row(1, { align = "start" })
			return
		end
		-- The last row, the bottom (Up from the first wraps round to it: the
		-- row past it the way the keys move is above, and bringing that in
		-- from the top leaves this one under the bottom edge).
		if key == nav[#nav] then
			list:scroll_to_row(#rows, { align = "end" })
			return
		end
		local head = head_of[key]
		if head then
			list:scroll_to_row(head, { align = "nearest" })
		end
		-- The next row the keys stop at, not the next row: between sections
		-- that's a thin spacer, which leaves this one under the fade.
		local sign = if (by or 0) < 0 then -1 else 1
		local at = table.find(nav, key)
		local following = if at then nav[at + sign] else nil
		local ahead = if following then index_of[following] else nil
		-- (This row first: of two calls in one turn the last wins, and it
		-- brings this row with it.)
		list:scroll_to_row(i, { align = "nearest" })
		list:scroll_to_row(ahead or math.clamp(i + sign, 1, #rows), { align = "nearest" })
	end
	local function move(by: number)
		local n = #nav
		if n == 0 then
			return
		end
		local at = table.find(nav, hi:peek() or "") or 0
		local i
		if math.abs(by) > 1 then
			-- A page's worth: to the end, not round it.
			i = math.clamp((if at == 0 then 1 else at) + by, 1, n)
		else
			if at == 0 then
				at = if by > 0 then 0 else n + 1
			end
			i = (at + by - 1) % n + 1
		end
		hi:set(nav[i])
		reveal(nav[i], by)
	end
	local function jump(to: number)
		local n = #nav
		if n > 0 then
			local key = nav[if to > 0 then n else 1]
			hi:set(key)
			reveal(key)
		end
	end

	-- Remembered as used: first among the recent.
	local function remember(id: string)
		local list = table.clone(recent:peek())
		local at = table.find(list, id)
		if at then
			table.remove(list, at)
		end
		table.insert(list, 1, id)
		while #list > RECENT * 2 do
			table.remove(list)
		end
		recent:set(list)
	end

	-- Opening a page over this one (loading it, if it loads), and going
	-- back to it.
	local function after_settle()
		ui.after(0, function()
			local h = hi:peek()
			if h then
				reveal(h)
			end
		end)
	end
	local function push(next: Page)
		local level = stack[#stack]
		level.query, level.hi, level.scope = query:peek(), hi:peek(), scope:peek()
		table.insert(stack, { page = next, query = "", hi = nil, scope = "all" })
		land = nil
		local function settle()
			-- Landing on its current choice, if it has one.
			for _, item in items_of(next) do
				if item.checked and item.checked() then
					land = "item:" .. item.id
					break
				end
			end
		end
		local waits = next.load ~= nil and loaded[next] == nil
		if not waits then
			settle()
		end
		ui.batch(function()
			query:set("")
			loading:set(waits)
			depth:set(#stack)
		end)
		if waits and next.load then
			next.load(function(items: { Item })
				loaded[next] = items
				-- Still the page shown: its rows take their shapes' place.
				if stack[#stack].page == next then
					settle()
					loading:set(false)
					after_settle()
				end
			end)
		end
		after_settle()
	end
	local function back()
		if #stack < 2 then
			return
		end
		table.remove(stack)
		local level = stack[#stack]
		land = level.hi
		ui.batch(function()
			query:set(level.query)
			scope:set(level.scope)
			loading:set(false)
			depth:set(#stack)
		end)
		after_settle()
	end

	-- Doing a row: Enter, a click; `alt`, ⌘Enter or a ⌘-click.
	local function activate(key: string?, alt: boolean)
		local r = if key then by_key[key] else nil
		if not r then
			return
		end
		if r.kind == "more" then
			scope:set(r.scope or "all")
			return
		end
		local item = r.item
		if not item then
			return
		end
		if alt then
			if item.alt then
				remember(item.id)
				open:set(false)
				item.alt.run()
			end
			return
		end
		if item.toggle then
			-- In place: the palette stays, the switch flips.
			item.toggle:toggle()
			remember(item.id)
			return
		end
		if item.page then
			remember(item.id)
			push(item.page())
			return
		end
		remember(item.id)
		open:set(false)
		if item.run then
			item.run()
		end
	end
	local function current(): Row?
		rev:get()
		local k = hi:get()
		return if k then by_key[k] else nil
	end
	local function set_scope(by: number)
		local at = 1
		for i, s in SCOPES do
			if s.id == scope:peek() then
				at = i
			end
		end
		scope:set(SCOPES[(at + by - 1) % #SCOPES + 1].id)
	end

	-- What's typed going somewhere at once (`typed`): there, nothing left
	-- typed behind it (so back doesn't go there again). (Out of the effect:
	-- a page opened inside one never hears its load.)
	ui.effect(function()
		local q = query:get()
		depth:get()
		local on = page()
		local typed = on.typed
		if q == "" or not typed then
			return
		end
		local to = typed(q)
		if to then
			ui.after(0, function()
				if query:peek() == q and page() == on then
					query:set("")
					push(to)
				end
			end)
		end
	end)
	local function caret_end()
		if field then
			local n = utf8.len(query:peek()) or #query:peek()
			field:set_selection(n, n)
		end
	end
	-- ⌘L: the page's path in the search, the caret after it, to type on
	-- from. (A menu's ⌘L hears it first in a window: it asks `type_path`.)
	local function type_path(): boolean
		if not open:peek() then
			return false
		end
		local here = page().path
		local at = if here then here() else nil
		if not at then
			return false
		end
		query:set(at)
		if field then
			local n = utf8.len(at) or #at
			field:focus()
			field:set_selection(n, n)
		end
		return true
	end
	table.insert(typing_paths, type_path)
	ui.on_cleanup(function()
		local i = table.find(typing_paths, type_path)
		if i then
			table.remove(typing_paths, i)
		end
	end)
	local function keys(e): boolean
		local k = e.key
		local cmd = e.meta == true or e.cmd == true
		if k == "ArrowDown" or (e.ctrl and (k == "n" or k == "j")) then
			if cmd then
				jump(1)
			else
				move(1)
			end
			return true
		elseif k == "ArrowUp" or (e.ctrl and (k == "p" or k == "k")) then
			if cmd then
				jump(-1)
			else
				move(-1)
			end
			return true
		elseif k == "PageDown" then
			move(8)
			return true
		elseif k == "PageUp" then
			move(-8)
			return true
		elseif k == "Enter" then
			-- Held, it does the row once.
			if not e["repeat"] then
				activate(hi:peek(), cmd)
			end
			return true
		elseif k == "Tab" then
			-- What's typed completed, first (a folder's name).
			local complete = page().complete
			local q = query:peek()
			local to = if complete and q ~= "" and not e.shift then complete(q) else nil
			if to and to ~= q then
				query:set(to)
				caret_end()
				return true
			end
			local r = by_key[hi:peek() or ""]
			if r and r.item and r.item.page and not e.shift then
				activate(r.key, false)
			elseif depth:peek() == 1 and scoped then
				set_scope(if e.shift then -1 else 1)
			end
			return true
		elseif k == "ArrowRight" then
			-- At the field's end, into the highlighted row's page.
			local r = by_key[hi:peek() or ""]
			if r and r.item and r.item.page and field then
				local _, b = field:selection()
				if b == nil or b >= utf8.len(query:peek()) then
					activate(r.key, false)
					return true
				end
			end
			return false
		elseif k == "ArrowLeft" and query:peek() == "" and depth:peek() > 1 then
			back()
			return true
		elseif k == "Backspace" and query:peek() == "" then
			if depth:peek() > 1 then
				back()
				return true
			elseif scope:peek() ~= "all" then
				scope:set("all")
				return true
			end
			return false
		elseif k == "Escape" then
			-- One step at a time: the search, then the page, then the palette.
			if query:peek() ~= "" then
				query:set("")
				return true
			elseif depth:peek() > 1 then
				back()
				return true
			end
			return false
		elseif cmd and (k == "1" or k == "2" or k == "3" or k == "4") and depth:peek() == 1 and scoped then
			scope:set(SCOPES[tonumber(k) :: number].id)
			return true
		elseif cmd and (k == "k" or k == "K") then
			open:set(false)
			return true
		elseif cmd and (k == "l" or k == "L") then
			return type_path()
		end
		return false
	end

	-- The search's row: the magnifier in the glyphs' slot (loading, the
	-- working ring), the pages opened (a chip each, going back to it), the
	-- field, its text on the labels' line.
	local crumbs = ui.row {
		gap = "$space.1",
		items = "center",
		shrink = 0,
		-- (A root without scopes is a step too: its title first, quiet while
		-- it's the one shown, a way back once another opens over it.)
		ui.show(
			function(): boolean
				return not scoped and depth:get() == 1
			end,
			ui.text {
				function(): string
					return props.root.title or ""
				end,
				literal = true,
				pl = "$zeron.space.tight",
				pr = "$space.1",
				text_size = "$font.size.sm",
				weight = "$font.weight.medium",
				color = "$text.tertiary",
			}
		),
		ui.each(ui.computed(function(): { string }
			local out = {}
			local d = depth:get()
			if scoped or d > 1 then
				for i = if scoped then 2 else 1, d do
					table.insert(out, stack[i].page.title)
				end
			end
			return out
		end), function(title: string)
			return ui.button {
				label = locale.t("ds.command_palette.back_from", { title = title }),
				tooltip = locale.s("ds.command_palette.back_tip"),
				tooltip_delay = 500,
				h = 24,
				pl = "$zeron.space.tight",
				pr = "$space.2",
				gap = "$space.1",
				items = "center",
				rounded = "$radius.lg",
				bg = "$zeron.secondary.bg",
				color = "$text.primary",
				hover = { bg = "$zeron.secondary.hover" },
				active = { bg = "$zeron.secondary.active" },
				text_size = "$font.size.sm",
				weight = "$font.weight.medium",
				on_click = back,
				ui.div { color = "$text.tertiary", Icon { name = "arrow-left", size = 12 } },
				ui.text { title, literal = true },
			}
		end),
	}
	local search = ui.row {
		h = SEARCH_H,
		w_full = true,
		shrink = 0,
		px = "$space.3",
		gap = "$space.2",
		items = "center",
		ui.div {
			w = SLOT,
			h = SLOT,
			shrink = 0,
			items = "center",
			justify = "center",
			color = "$text.tertiary",
			ui.show(function(): boolean
				return loading:get() or searching:get()
			end, status.StatusIcon { kind = "working", size = 14 }, Icon { name = "palette-search", size = 16 }),
		},
		ui.show(function(): boolean
			-- (A root without scopes is a step: its title shows.)
			return depth:get() > 1 or not scoped
		end, crumbs),
		ui.input(EditorMenu.on({
			id = "palette-search",
			value = query,
			label = locale.t("ds.command_palette.search"),
			placeholder = function(): string
				depth:get()
				return page().placeholder or locale.s("ds.command_palette.placeholder")
			end,
			placeholder_color = "$text.tertiary",
			grow = 1,
			h = 28,
			px = 0,
			border = 0,
			bg = "#00000000",
			focus = { border = 0, outline = 0 },
			text_size = "$font.size.lg",
			on_key = keys,
			on_mount = function(el)
				field = el
				ui.after(0, function()
					el:focus()
				end)
			end,
		})),
	}

	-- The scopes (at the root): each its count, the one chosen in the
	-- menus' selected wash, its text on the glyphs' edge.
	local scopes: { Node } = {}
	for i, s in SCOPES do
		local function chosen(): boolean
			return scope:get() == s.id
		end
		table.insert(
			scopes,
			ui.button {
				label = locale.t(s.label),
				tooltip = ui.shortcut_text(`cmd+{i}`),
				tooltip_delay = 500,
				h = 26,
				px = "$space.2",
				gap = "$zeron.space.tight",
				items = "center",
				rounded = "$radius.lg",
				text_size = "$font.size.sm",
				weight = "$font.weight.medium",
				variants = scope_chip { chosen = chosen },
				on_click = function()
					scope:set(s.id)
				end,
				ui.text { locale.t(s.label) },
				ui.text {
					function(): string
						return count(counts:get()[s.id] or 0)
					end,
					text_size = "$font.size.xs",
					color = "$text.tertiary",
					tabular = true,
				},
			}
		)
	end
	local scope_items: { [any]: any } = {
		h = SCOPES_H,
		w_full = true,
		px = "$space.1",
		gap = "$zeron.space.hair",
		items = "center",
	}
	for _, node in scopes do
		table.insert(scope_items, node)
	end
	table.insert(scope_items, ui.div { grow = 1 })
	table.insert(
		scope_items,
		ui.row {
			pr = "$space.2",
			gap = "$zeron.space.tight",
			items = "center",
			Kbd { keys = "tab", tone = "quiet" },
			ui.text { locale.t("ds.command_palette.next_scope"), text_size = "$font.size.xs", color = "$text.tertiary" },
		}
	)
	local scope_row = ui.show(
		function(): boolean
			return depth:get() == 1 and scoped
		end,
		ui.column { w_full = true, shrink = 0, ui.row(scope_items), rule() }
	)

	-- A row's glyph, in its slot: a session's agent, a project's tile, a
	-- mark, an icon; or the slot empty (the label still on the line).
	local function glyph(item: Item): Node
		local s = item.session
		local inner: Node? = nil
		if s then
			inner = if s.agent
				then Mark { name = s.agent, size = 14, bare = true }
				else ui.div { w = 5, h = 5, rounded = "$radius.full", bg = "$text.disabled" }
		elseif item.hue or item.src or item.emoji then
			inner = Tile { label = item.label, hue = item.hue, src = item.src, emoji = item.emoji, size = SLOT }
		elseif item.mark then
			inner = Mark { name = item.mark, size = 14, bare = true }
		elseif item.icon then
			inner = Icon { name = item.icon, size = 16 }
		end
		return ui.div {
			w = SLOT,
			h = SLOT,
			shrink = 0,
			items = "center",
			justify = "center",
			color = "$text.secondary",
			opacity = if s and s.archived then 0.5 else 1,
			inner,
		}
	end
	-- A session's corner: its state's glyph in its hue, else when it moved.
	local function standing(s: Session): Node?
		if s.status then
			local k = status.KINDS[s.status]
			return ui.div {
				shrink = 0,
				w = 16,
				h = 16,
				items = "center",
				justify = "center",
				color = `$zeron.status.{s.status}`,
				tooltip = k.long,
				label = k.long,
				if k.glyph then status.StatusIcon { kind = s.status, size = 14 } else status.StatusDot { kind = s.status, size = 6 },
			}
		end
		local t = s.time
		local said = if type(t) == "number" then status.ago(os.time() - t) else t
		if not said then
			return nil
		end
		return ui.text { said, shrink = 0, text_size = "$font.size.xs", color = "$text.tertiary", tabular = true }
	end
	local function tag(text: string): Node
		return ui.row {
			shrink = 0,
			h = 16,
			px = "$space.1",
			items = "center",
			rounded = "$radius.sm",
			bg = "$zeron.secondary.bg",
			ui.text { text, text_size = "$font.size.xs", color = "$text.tertiary" },
		}
	end
	-- A text marked where it matched, following the searches: built again
	-- only when its marks change (the row around it stays).
	local function live(id: string, key: string, text: string, style: TextStyle): Node
		local function marks(): string
			rev:get()
			local hits = hits_of[id]
			local at = if hits then hits[key] else nil
			return if at then table.concat(at, ",") else ""
		end
		return ui.presence(marks, function(sig: string)
			local hits = hits_of[id]
			return marked(text, if sig ~= "" and hits then hits[key] else nil, style)
		end, { grow = style.grow, shrink = style.shrink or 1, min_w = 0 })
	end

	-- A row, as its kind shows it. The highlight is one wash that slides to
	-- the row the keys (or the pointer) reach.
	-- A row's corner: concentric with the card, through its edge and the
	-- list's inset.
	local row_corner = control.nest("$radius.xl", 1, "$space.1")
	local function row(r: Row): Node
		if r.kind == "pad" then
			return ui.div { w_full = true, h = r.h }
		end
		if r.kind == "head" then
			-- (As tall as its place in the list, which it fills: the same
			-- title is the first one (shorter) or a later one as a search
			-- reorders the sections, and keeps its row as it moves.)
			return ui.row {
				w_full = true,
				h_full = true,
				px = "$space.3",
				pb = "$zeron.space.tight",
				gap = "$space.2",
				items = "end",
				Text {
					function(): string
						return title_text(r.title or "")
					end,
					style = "label",
					tone = "tertiary",
					grow = 1,
				},
				-- (A count where it says something: a long section's.)
				if r.count and r.count > 9
					then ui.text { count(r.count), text_size = "$font.size.xs", color = "$text.disabled", tabular = true }
					else nil,
			}
		end
		if r.kind == "ghost" then
			-- A row's shape while its page loads: a glyph and a label of a few
			-- lengths, breathing one after another.
			local widths = { 168, 120, 212, 144, 96 }
			local breath = ui.use_state(0.4)
			return ui.div {
				w_full = true,
				h = r.h,
				px = "$space.1",
				on_mount = function()
					-- (Still, with motion reduced.)
					if motion.reduced_now() then
						return
					end
					ui.after((r.count or 1) * 90, function()
						anime.animate(breath, { value = 0.9, duration = 700, ease = "inOutSine", loop = true, alternate = true })
					end)
				end,
				ui.row {
					w_full = true,
					h = r.h - gap(),
					px = "$space.2",
					gap = "$space.2",
					items = "center",
					opacity = breath,
					ui.div { w = SLOT, h = SLOT, shrink = 0, rounded = "$radius.sm", bg = "$zeron.secondary.bg" },
					ui.div { w = widths[r.count or 1], h = 10, rounded = "$radius.sm", bg = "$zeron.secondary.bg" },
				},
			}
		end
		local key = r.key
		local function lit(): boolean
			return hi:get() == key
		end
		local content: { Node } = {}
		local tall = false
		local label: string? = nil
		if r.kind == "more" then
			label = locale.s("ds.command_palette.show_all_n", { n = count(r.count or 0), what = string.lower(title_text(r.title or "")) })
			table.insert(content, ui.div { w = SLOT, h = SLOT, shrink = 0, items = "center", justify = "center", color = "$text.tertiary", Icon { name = "list", size = 14 } })
			table.insert(content, FadeText { label, grow = 1, text_size = "$font.size.md", color = "$text.secondary" })
			table.insert(content, Kbd { keys = "tab", tone = "quiet" })
		else
			local item = r.item :: Item
			label = item.label
			local s = item.session
			if s then
				tall = true
				local line: { Node } = {
					live(item.id, "label", item.label, { text_size = "$font.size.md", color = if s.archived then "$text.secondary" else "$text.primary", grow = 1 }),
				}
				if s.archived then
					table.insert(line, tag(locale.s("ds.command_palette.archived")))
				end
				if s.pr then
					table.insert(line, pr_badge(s.pr))
				end
				local corner = standing(s)
				if corner then
					table.insert(line, corner)
				end
				-- The second line: its project, branch and device, as the
				-- sidebar's regular rows read, each marked where it matched.
				local detail: { Node } = {}
				local function part(icon: Node, key: string, text: string, last: boolean)
					table.insert(
						detail,
						ui.row {
							gap = "$space.1",
							items = "center",
							shrink = if last then 1 else 0,
							min_w = 0,
							icon,
							live(item.id, key, text, { text_size = "$font.size.xs", color = "$text.tertiary", shrink = if last then 1 else 0 }),
						}
					)
				end
				if s.project then
					part(Tile { label = s.project, hue = s.hue, src = s.src, emoji = s.emoji, size = 12 }, "project", s.project, s.branch == nil and s.device == nil)
				end
				if s.branch then
					part(ui.div { color = "$text.tertiary", Icon { name = "git-branch", size = 11 } }, "branch", s.branch, s.device == nil)
				end
				if s.device then
					part(ui.div { color = "$text.tertiary", Icon { name = if s.device == "Cloud" then "cloud" else "monitor", size = 11 } }, "device", s.device, true)
				end
				table.insert(content, glyph(item))
				local detail_row: { [any]: any } = { w_full = true, h = 16, gap = "$space.3", items = "center", overflow = "hidden" }
				for _, node in detail do
					table.insert(detail_row, node)
				end
				local line_row: { [any]: any } = { w_full = true, h = SLOT, gap = "$space.2", items = "center" }
				for _, node in line do
					table.insert(line_row, node)
				end
				table.insert(content, ui.column { grow = 1, min_w = 0, gap = "$zeron.space.hair", ui.row(line_row), ui.row(detail_row) })
			else
				table.insert(content, glyph(item))
				table.insert(content, live(item.id, "label", item.label, { text_size = "$font.size.md", color = "$text.primary", grow = 1 }))
				if item.hint then
					table.insert(content, ui.text { item.hint, shrink = 0, text_size = "$font.size.sm", color = "$text.tertiary" })
				end
				if item.shortcut then
					table.insert(content, Kbd { keys = item.shortcut, tone = "quiet" })
				end
				if item.checked then
					-- A menu's check, so a choice reads the same in both.
					table.insert(content, Menu.check(item.checked))
				end
				if item.toggle then
					-- (The row takes the click: the switch only shows it.)
					table.insert(content, ui.div { shrink = 0, scale = 0.75, pointer_events = "none", Switch { value = item.toggle, label = item.label } })
				end
				if item.page then
					table.insert(content, Menu.chevron())
				end
			end
		end
		local body: { [any]: any } = {
			w_full = true,
			h = r.h - gap(),
			relative = true,
			px = "$space.2",
			py = if tall then "$space.2" else nil,
			gap = "$space.2",
			items = if tall then "start" else "center",
			rounded = row_corner,
			cursor = "pointer",
			role = "option",
			label = label,
			selected = lit,
			-- Motion alone moves the highlight: rows scrolling under a resting
			-- pointer leave the keys' place alone.
			on_pointer_move = function()
				if hi:peek() ~= key then
					hi:set(key)
				end
			end,
			on_click = function(e)
				activate(key, e ~= nil and e.meta == true)
				-- (Still open, the keys stay the search's: a row clicked takes
				-- no focus of its own.)
				if open:peek() and field then
					field:focus()
				end
			end,
			-- The highlight, under the row's content: one shared wash with a
			-- hairline ring, sliding from the row it leaves.
			ui.presence(function(): string?
				return if lit() then "on" else nil
			end, function()
				return ui.div {
					layout_id = "palette-highlight",
					layout_duration = SLIDE_MS,
					layout_ease = SLIDE_EASE,
					w_full = true,
					h_full = true,
					rounded = row_corner,
					bg = "$zeron.menu.selected",
					border = 1,
					border_color = "$border.subtle",
				}
			end, { absolute = true, left = 0, right = 0, top = 0, bottom = 0 }),
		}
		for _, node in content do
			table.insert(body, node)
		end
		return ui.div { w_full = true, h = r.h, px = "$space.1", ui.row(body) }
	end

	local list = ui.list {
		id = "palette-list",
		ref = list_ref,
		w_full = true,
		h = motion.tween(ui.computed(list_h), GROW),
		offset = offset,
		heights = function(): { number }
			rev:get()
			return heights
		end,
		key = function(i: number): string
			rev:get()
			local r = rows[i]
			return if r then r.key else `gone:{i}`
		end,
		row = function(key: string): Node
			local r = by_key[key]
			if not r then
				return ui.div { w_full = true, h = 1 }
			end
			return row(r)
		end,
		-- Each edge fades only where there is more to scroll that way.
		fade = function()
			local total = total_h:get()
			local o = offset:get()
			local most = math.max(0, total - math.min(view_max(), total))
			return { math.min(o, 20), math.min(math.max(0, most - o), 20) }
		end,
	}

	-- Nothing to list: the list's room as the dropdown's quiet field of
	-- characters, clearing in the middle for what is missing and where else
	-- to look.
	local function note(): Note
		local q = query:get()
		local d = depth:get()
		local p = page()
		if q == "" then
			return p.empty or { title = locale.s("ds.command_palette.empty.nothing_yet") }
		end
		local c = counts:get()
		if d == 1 and scoped and scope:get() ~= "all" and c.all > 0 then
			return {
				title = locale.s("ds.command_palette.empty.nothing_in_scope"),
				hint = locale.s("ds.command_palette.empty.elsewhere", { n = count(c.all) }),
			}
		elseif d > 1 then
			return {
				title = locale.s("ds.command_palette.empty.no_matches"),
				hint = locale.s("ds.command_palette.empty.nothing_in_page", { title = p.title, q = q }),
			}
		end
		return {
			title = locale.s("ds.command_palette.empty.no_matches"),
			hint = locale.s("ds.command_palette.empty.nothing_matches", { q = q }),
		}
	end
	local empty = ui.column {
		w_full = true,
		h = NOTE_H,
		relative = true,
		overflow = "hidden",
		fade = { 24, 24 },
		px = "$space.6",
		gap = "$space.2",
		items = "center",
		justify = "center",
		ui.div {
			absolute = true,
			left = 0,
			right = 0,
			top = 0,
			bottom = 0,
			pointer_events = "none",
			bg_shader = ascii.QUIET,
			shader_params = ascii.quiet("$text.tertiary", 0.12, { 220, 40 }),
			on_mount = ascii.drift,
		},
		ui.text {
			function(): string
				return string.upper(note().title)
			end,
			font = "$font.mono",
			text_size = "$font.size.xs",
			letter_spacing = 1,
			color = "$text.secondary",
		},
		ui.show(function(): boolean
			return note().hint ~= nil
		end, Text {
			function(): string
				return note().hint or ""
			end,
			style = "caption",
			tone = "tertiary",
			align = "center",
			wrap = true,
			max_w = 380,
		}),
	}

	-- The footer, a legend of the keys: what Enter and ⌘Enter do to the
	-- highlighted row, and the way out.
	local function verb(): string
		local r = current()
		if not r then
			return locale.s("ds.command_palette.verb.open")
		end
		if r.kind == "more" then
			return locale.s("ds.command_palette.verb.show_all")
		end
		local item = r.item :: Item
		if item.toggle then
			return locale.s(if item.toggle:get() then "ds.command_palette.verb.turn_off" else "ds.command_palette.verb.turn_on")
		end
		if item.page then
			return locale.s("ds.command_palette.verb.open")
		end
		return item.verb or locale.s(if item.session then "ds.command_palette.verb.open" else "ds.command_palette.verb.run")
	end
	local function alt_label(): string?
		local r = current()
		if r and r.item and r.item.alt then
			return r.item.alt.label
		end
		return nil
	end
	local function way_out(): string
		if query:get() ~= "" then
			return locale.s("ds.command_palette.clear")
		end
		return locale.s(if depth:get() > 1 then "ds.command_palette.back" else "ds.command_palette.close")
	end
	local function legend(keys: Node, what: string | () -> string, strong: boolean?): Node
		return ui.row {
			gap = "$zeron.space.tight",
			items = "center",
			shrink = 0,
			keys,
			ui.text { what, text_size = "$font.size.xs", color = if strong then "$text.secondary" else "$text.tertiary" },
		}
	end
	local footer = ui.row {
		h = FOOT_H,
		w_full = true,
		shrink = 0,
		px = "$space.3",
		gap = "$space.4",
		items = "center",
		ui.show(function(): boolean
			return current() ~= nil
		end, legend(Kbd { keys = "enter" }, verb, true)),
		ui.show(function(): boolean
			return alt_label() ~= nil
		end, legend(Kbd { keys = "cmd+enter" }, function(): string
			return alt_label() or ""
		end)),
		ui.div { grow = 1 },
		ui.show(function(): boolean
			return shown_rows:get() > 1
		end, legend(ui.row { gap = "$zeron.space.hair", Kbd { keys = "up" }, Kbd { keys = "down" } }, function(): string
			return locale.s("ds.command_palette.navigate")
		end)),
		legend(Kbd { text = "esc" }, way_out),
	}

	return ui.column {
		id = "command-palette",
		w_full = true,
		gap = 0,
		rounded = "$radius.xl",
		overflow = "hidden",
		-- Frosted: the palette's glass over a blur of the page under it.
		-- Opaque: a solid card.
		bg = function()
			return if material.see_through() then "$zeron.palette.tint" else "$zeron.palette.solid"
		end,
		backdrop_blur = function()
			return if material.see_through() then "$zeron.menu.blur" else 0
		end,
		border = 1,
		border_color = "$border",
		inset_shadow = "$zeron.highlight",
		shadow = "$zeron.menu.shadow",
		-- Opening: a short scale up and fade in (each time: the card is
		-- built as the palette opens).
		on_mount = function(el)
			local opening: ElementParams = { opacity = { 0, 1 }, scale = { 0.97, 1 }, duration = OPEN.duration, ease = OPEN.ease }
			anime.animate(el, motion.params(opening))
		end,
		search,
		rule(),
		scope_row,
		ui.show(function(): boolean
			return shown_rows:get() > 0 or loading:get()
		end, list, empty),
		rule(),
		footer,
	}
end)

-- Each open palette's ⌘L, the newest last.
typing_paths = {}

local Palette = ui.component(function(props: Props): Node
	local recent = props.recent or ui.use_state({} :: { string })
	return ui.modal {
		open = props.open,
		label = locale.t("ds.command_palette.label"),
		-- Its top held still, so only its foot moves.
		placement = { x = "center", y = "start", inset = { TOP, 0, 0, 0 } },
		w = W,
		p = 0,
		gap = 0,
		-- The card draws its own glass (so it can open with a scale); the
		-- panel under it is bare.
		bg = "#00000000",
		rounded = "$radius.xl",
		build = function()
			return Body { open = props.open, root = props.root, recent_state = recent }
		end,
	}
end)

-- ⌘L as a menu key hears it (a menu's key comes before the field's): the
-- open palette's page path typed, if it has one; whether it did.
local function type_path(): boolean
	local f = typing_paths[#typing_paths]
	return f ~= nil and f()
end

return setmetatable({ type_path = type_path }, {
	__call = function(_, props: Props): Node
		return Palette(props)
	end,
})
