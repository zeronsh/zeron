-- The address bar's suggestions: as an address or words are typed, a list
-- under the field of where Enter could go, and the field finishing the
-- likeliest site as you type (a browser's inline completion).
--
-- What's offered, top down:
--   1. What Enter does now: the site the field has finished ("github.com",
--      the rest of it selected), else the address typed ("Go to
--      example.com"), else a search for the words on the engine chosen.
--   2. The project's dev servers whose name, port or address match (the
--      start page's previews: the pages a chat is most likely after).
--   3. Pages from history (this account's: browser_data), ranked first by
--      how they match (the address starts with it, a part of the site's
--      name does, each word typed starts a word of the address or title;
--      letters inside a word never match), then by how often and how
--      lately they were visited. What matched is lit, at word starts only.
--
-- Keys, as in a browser: ↓ ↑ move through the list, the field showing the
-- row lit; Enter goes; Tab takes the finished site; Escape puts back what
-- was typed and closes the list (again, the field's own: the page's
-- address); Backspace takes the finished part away without finishing it
-- again; Shift-Backspace (or the row's ×) lets a page out of history. The
-- list opens only as you type (never as the field takes the keyboard), and
-- closes as the field is left or a row is taken.
--
--   local s = suggest.use { value = address, input = address_ref, profile = …,
--     engine = …, previews = …, choose = function(url, text) … end, id = tab_id }
--   ds.TextField { …, on_key = s.keys, on_focus_change = s.on_focus_change,
--     role = "combobox", expanded = s.open, active_descendant = s.descendant }
--   ds.TextField { …, joined = s.open }    -- the field opens into the list
--   s.list(field_box_ref, tab_root_ref, corner)    -- last in the tab's (relative) column
--   s.quiet(text)   before the browser sets the field itself (not typing)
--
-- The ranking, the finishing and the matched runs are plain functions
-- (`rank`, `inline`, `runs`), for tests.

local ds = require("@ds")
local Icon = require("@ds/icon")
local Tile = require("@ds/tile")
local IconButton = require("@ds/icon_button").IconButton
local data = require("@app/screens/panel/browser_data")
local api = require("@app/screens/panel/browser_api")
local motion = require("@ds/motion")

local M = {}

-- Most rows shown, and their height (a menu row's, a little taller: a
-- title and its address side by side, a 16-point tile).
local MOST = 8
local ROW = 32
-- A row's wash: the joined shape's corner (the field's, a pill's half
-- height at the small size: 14) less the rows' 4 inset.
local ROW_RADIUS = 10

export type Row = {
	-- Its identity in the list (kept across keystrokes: a row that stays
	-- is the same row).
	key: string,
	kind: "complete" | "go" | "search" | "preview" | "visit",
	-- Where taking it goes.
	url: string,
	-- What the field shows while it's lit (from the keys).
	fill: string,
	title: string,
	detail: string?,
	-- A site's icon file, and the name its initial is taken from.
	icon: string?,
	site: string?,
	-- A page from history: it can be let go.
	visit: boolean?,
}

-- Matching -----------------------------------------------------------------

local function trim(s: string): string
	return string.match(s, "^%s*(.-)%s*$") or s
end

local function words_of(q: string): { string }
	local out = {}
	for w in string.gmatch(string.lower(q), "%S+") do
		table.insert(out, w)
	end
	return out
end

local function host_of(url: string): string
	return string.match(url, "^%a[%w+.-]*://([^/?#]+)") or ""
end

local function starts(s: string, p: string): boolean
	return string.sub(s, 1, #p) == p
end

-- Whether `w` starts a word of `hay` (its start, or after anything not a
-- letter or digit: "re" starts "react.dev/reference" twice, never
-- "datastore"), as a browser's address bar matches.
local function starts_a_word(hay: string, w: string): boolean
	local from = 1
	while true do
		local at = string.find(hay, w, from, true)
		if not at then
			return false
		end
		if at == 1 or not string.match(string.sub(hay, at - 1, at - 1), "[%w]") then
			return true
		end
		from = at + 1
	end
end

-- How a visit matches: 3 its address starts with what's typed, 2 a part of
-- its site's name does ("apple" in developer.apple.com), 1 each word typed
-- starts a word of its address or title, 0 not at all (letters inside a
-- word don't count: a short query would light half of history).
local function match_class(v: data.Visit, q: string, words: { string }): number
	local short = string.lower(data.short(v.url))
	if starts(short, q) then
		return 3
	end
	local host = string.match(short, "^[^/]+") or short
	if #words <= 1 then
		for part in string.gmatch(host, "[^.:]+") do
			if starts(part, words[1] or q) then
				return 2
			end
		end
	end
	local hay = `{short} {string.lower(v.title or "")}`
	for _, w in words do
		if not starts_a_word(hay, w) then
			return 0
		end
	end
	return 1
end

-- How much a visit counts: visited more, and lately.
local function frecency(v: data.Visit, now: number): number
	local age = now - v.at
	local lately = if age < 86400 then 1 elseif age < 7 * 86400 then 0.7 elseif age < 30 * 86400 then 0.5 else 0.3
	return (1 + math.log(v.count or 1, 2)) * lately
end

-- The visits matching `text`, the best first, at most `n`.
function M.rank(visits: { data.Visit }, text: string, n: number, now: number?): { data.Visit }
	local q = string.lower(trim(text))
	if q == "" then
		return {}
	end
	q = string.gsub(q, "^https?://", "")
	q = string.gsub(q, "^www%.", "")
	local words = words_of(q)
	local t = now or os.time()
	local scored: { { v: data.Visit, s: number } } = {}
	for _, v in visits do
		local class = match_class(v, q, words)
		if class > 0 then
			-- (The match decides; how often and how lately order within it.)
			table.insert(scored, { v = v, s = class * 10 + frecency(v, t) })
		end
	end
	table.sort(scored, function(a, b)
		if a.s ~= b.s then
			return a.s > b.s
		end
		return a.v.at > b.v.at
	end)
	local out: { data.Visit } = {}
	for i = 1, math.min(n, #scored) do
		out[i] = scored[i].v
	end
	return out
end

-- The site (or, with a `/` typed, the page) the field would finish `typed`
-- as: what to add after it, and where it goes. Nil when nothing visited
-- starts with it, or with words, a scheme, or nothing typed.
function M.inline(visits: { data.Visit }, typed: string, now: number?): (string?, string?)
	if typed == "" or string.find(typed, "%s") or string.find(typed, "://", 1, true) then
		return nil, nil
	end
	local q = string.lower(typed)
	local lead = ""
	if starts(q, "www.") then
		lead = "www."
		q = string.sub(q, 5)
	end
	if q == "" then
		return nil, nil
	end
	local t = now or os.time()
	local path = string.find(q, "/", 1, true) ~= nil
	-- Each site (or page) that starts with it, by how much its visits count.
	local best: string? = nil
	local best_s = -1
	local best_url: string? = nil
	local sums: { [string]: number } = {}
	local root: { [string]: string } = {}
	local any_url: { [string]: string } = {}
	for _, v in visits do
		local short = string.lower(data.short(v.url))
		local target = if path then short else (string.match(short, "^[^/]+") or short)
		if starts(target, q) and #target > #q then
			sums[target] = (sums[target] or 0) + frecency(v, t)
			if not path then
				local scheme, host = string.match(v.url, "^(https?)://([^/?#]+)")
				if scheme and host and not any_url[target] then
					any_url[target] = `{scheme}://{host}/`
				end
				if data.short(v.url) == target then
					root[target] = v.url
				end
			else
				any_url[target] = any_url[target] or v.url
			end
			if sums[target] > best_s then
				best, best_s = target, sums[target]
			end
		end
	end
	if not best then
		return nil, nil
	end
	best_url = root[best] or any_url[best]
	return string.sub(best, #q + 1), best_url
end

-- `text` cut into runs, each matched or not by the words of `q` (bytes,
-- whole characters: the words are typed text).
export type Run = { text: string, on: boolean }
function M.runs(text: string, q: string): { Run }
	local lower = string.lower(text)
	local mark: { [number]: boolean } = {}
	for _, w in words_of(q) do
		local from = 1
		while true do
			local s, e = string.find(lower, w, from, true)
			if not s then
				break
			end
			-- (Only where it starts a word, as it matched.)
			if s == 1 or not string.match(string.sub(lower, s - 1, s - 1), "[%w]") then
				for i = s, e do
					mark[i] = true
				end
			end
			from = s + 1
		end
	end
	local out: { Run } = {}
	local run, on = "", false
	for i = 1, #text do
		local m = mark[i] == true
		if m ~= on and run ~= "" then
			table.insert(out, { text = run, on = on })
			run = ""
		end
		on = m
		run ..= string.sub(text, i, i)
	end
	if run ~= "" then
		table.insert(out, { text = run, on = on })
	end
	return out
end

-- The engine's name, for "Search … for".
local function engine_name(): string
	local id = data.engine:peek()
	if id == "custom" then
		local host = host_of(data.custom_engine:peek())
		return if host ~= "" then host else ui.t("settings.browser.search_engine.custom"):get()
	end
	for _, e in data.ENGINES do
		if e.id == id then
			return e.name
		end
	end
	return data.ENGINES[1].name
end

-- The rows for what's typed (and the site the field finished, if it did).
function M.rows(opts: { typed: string, done: string?, done_url: string?, profile: string, previews: { any } }): { Row }
	local typed = trim(opts.typed)
	local out: { Row } = {}
	if typed == "" then
		return out
	end
	local taken: { [string]: boolean } = {}
	local function add(r: Row)
		if #out < MOST and not taken[r.url] then
			taken[r.url] = true
			table.insert(out, r)
		end
	end
	local history = data.history(opts.profile)
	-- 1. What Enter does.
	if opts.done and opts.done_url then
		local full = opts.typed .. opts.done
		local host = host_of(opts.done_url)
		add({
			key = "top:complete",
			kind = "complete",
			url = opts.done_url,
			fill = full,
			title = full,
			icon = data.icon_of(opts.profile, host),
			site = data.site_name(host),
		})
	else
		local url, _, searched = api.resolve(typed, data.engine_url())
		if url and not searched then
			local host = host_of(url)
			add({
				key = "top:go",
				kind = "go",
				url = url,
				fill = typed,
				title = typed,
				detail = ui.t("panel.browser.suggest.go"):get(),
				icon = data.icon_of(opts.profile, host),
				site = data.site_name(host),
			})
		elseif url then
			add({
				key = "top:search",
				kind = "search",
				url = url,
				fill = typed,
				title = typed,
				detail = ui.t("panel.browser.suggest.search", { engine = engine_name() }):get(),
			})
		end
	end
	-- 2. The project's dev servers.
	local words = words_of(typed)
	for _, sv in opts.previews do
		local hay = string.lower(`{sv.name or ""} {sv.url or ""} :{sv.port or ""} localhost`)
		local all = true
		for _, w in words do
			if not string.find(hay, w, 1, true) then
				all = false
				break
			end
		end
		if all and type(sv.url) == "string" then
			add({
				key = `preview:{sv.id or sv.url}`,
				kind = "preview",
				url = sv.url,
				fill = data.short(sv.url),
				title = tostring(sv.name or data.short(sv.url)),
				detail = data.short(sv.url),
			})
		end
	end
	-- 3. History.
	for _, v in M.rank(history, typed, MOST) do
		add({
			key = `visit:{v.url}`,
			kind = "visit",
			url = v.url,
			fill = data.short(v.url),
			title = if v.title and v.title ~= "" then v.title else data.short(v.url),
			detail = data.short(v.url),
			icon = v.icon,
			site = data.site_name(host_of(v.url)),
			visit = true,
		})
	end
	return out
end

-- Text that fades at its end where it doesn't fit (ds/fade_text's way),
-- its matched runs in the full ink a weight up.
local MATCH_WEIGHT = 560
local Matched = ui.component(function(props: { runs: { Run }, ink: string, size: string, grow: number?, shrink: number? }): Node
	local clip, text = ui.ref(), ui.ref()
	local spans: { [any]: any } = {}
	for _, r in props.runs do
		table.insert(
			spans,
			if r.on
				then ui.span { r.text, literal = true, color = "$text.primary", weight = MATCH_WEIGHT }
				else ui.span { r.text, literal = true }
		)
	end
	spans.ref = text
	spans.shrink = 0
	spans.text_size = props.size
	spans.color = props.ink
	return ui.row {
		ref = clip,
		min_w = 0,
		grow = props.grow,
		shrink = props.shrink or 1,
		overflow = "hidden",
		items = "center",
		fade_x = function()
			local over = (text.width or 0) > (clip.width or 0) + 0.5
			return { 0, if over then 24 else 0 }
		end,
		ui.rich(spans),
	}
end)

export type Options = {
	value: State<string>,
	input: Ref,
	profile: string,
	previews: () -> { any },
	-- Taking a row: where it goes, and what the field shows meanwhile.
	choose: (url: string, shown: string) -> (),
	id: string,
}

function M.use(opts: Options): { [string]: any }
	local value = opts.value
	-- What was typed, the list for it, the row lit (0: none), and whether
	-- the list is out.
	local typed = ui.use_state("")
	local shown = ui.use_state({} :: { Row })
	local active = ui.use_state(1)
	local out = ui.use_state(false)
	-- The field's last words not typed: ours (a finish, a row lit) or the
	-- browser's (a page's address arriving, Escape's), told by `quiet`.
	-- Every other change while the field has the keyboard is typing (text
	-- comes in as text, not always as keys). Whether the last key erased.
	local ours: string? = nil
	local erasing = false
	local focused = false

	local function el(): any
		return opts.input:peek()
	end
	-- The field's words set by us (a finish, a row lit, what was typed put
	-- back), through the input itself as typing would, and its selection
	-- with them in the same breath: set through the state, the input took
	-- the words a frame later and put the caret at their end, after the
	-- selection was made, and Backspace then ate a letter of the finish.
	local function set_field(text: string, from: number?, to: number?)
		ours = text
		local n = utf8.len(text) or #text
		local e = el()
		if e and e:select_all() and e:replace_selection(text) then
			e:set_selection(from or n, to or n)
			-- (The state follows the input; set too, for a host whose input
			-- doesn't write back at once.)
			if value:peek() ~= text then
				value:set(text)
			end
			return
		end
		value:set(text)
		ui.after(0, function()
			local later = el()
			if later then
				later:set_selection(from or n, to or n)
			end
		end)
	end
	local function close()
		if out:peek() then
			out:set(false)
		end
	end

	-- What's typed, read again: the rows, and the site finished in the field
	-- when the caret is at its end and a character was just added.
	local function refresh(text: string, finish: boolean)
		typed:set(text)
		local done, done_url = nil, nil
		if finish then
			done, done_url = M.inline(data.history(opts.profile), text)
		end
		local rows = M.rows({ typed = text, done = done, done_url = done_url, profile = opts.profile, previews = opts.previews() })
		shown:set(rows)
		active:set(if #rows > 0 then 1 else 0)
		out:set(#rows > 0)
		if done and done ~= "" then
			local n = utf8.len(text) or #text
			set_field(text .. done, n, n + (utf8.len(done) or #done))
		end
	end

	-- The field's words changing: typed, or not (ours, the browser's, or
	-- with the keyboard elsewhere: the list stays shut).
	ui.effect(function()
		local now = value:get()
		if now == ours then
			return
		end
		ours = nil
		if not focused then
			return
		end
		local finish = not erasing
		erasing = false
		-- (Only with the caret at the end: a word mended mid-address isn't
		-- finished from the end.)
		local e = el()
		if finish and e then
			local _, at = e:selection()
			local n = utf8.len(now) or #now
			if at and at < n then
				finish = false
			end
		end
		refresh(now, finish)
	end)

	local function light(i: number)
		local rows = shown:peek()
		if #rows == 0 then
			return
		end
		i = math.clamp(i, 1, #rows)
		active:set(i)
		local r = rows[i]
		-- (The first row puts back what was typed, finished as it was.)
		if i == 1 and r.kind == "complete" then
			local t = typed:peek()
			local n = utf8.len(t) or #t
			set_field(r.fill, n, utf8.len(r.fill) or #r.fill)
		else
			set_field(r.fill)
		end
	end

	local function take(r: Row)
		close()
		ours = r.url
		opts.choose(r.url, r.fill)
	end

	local function forget(r: Row)
		if not r.visit then
			return
		end
		data.forget(opts.profile, r.url)
		local rows = {}
		for _, x in shown:peek() do
			if x.key ~= r.key then
				table.insert(rows, x)
			end
		end
		shown:set(rows)
		active:set(math.clamp(active:peek(), if #rows > 0 then 1 else 0, #rows))
		out:set(#rows > 0)
	end

	local function keys(e: any): boolean?
		local k = e.key
		local open = out:peek() and #shown:peek() > 0
		if open and (k == "ArrowDown" or k == "ArrowUp") then
			light(active:peek() + (if k == "ArrowDown" then 1 else -1))
			return true
		end
		if open and k == "Enter" then
			local r = shown:peek()[active:peek()]
			if r then
				take(r)
				return true
			end
		end
		if open and k == "Escape" then
			set_field(typed:peek())
			close()
			return true
		end
		if open and k == "Tab" and not e.shift then
			local r = shown:peek()[1]
			if r and r.kind == "complete" and active:peek() == 1 then
				set_field(r.fill)
				return true
			end
			close()
			return nil
		end
		if open and e.shift and (k == "Backspace" or k == "Delete") then
			local r = shown:peek()[active:peek()]
			if r and r.visit then
				forget(r)
				return true
			end
		end
		if k == "ArrowDown" or k == "ArrowUp" or k == "ArrowLeft" or k == "ArrowRight" or k == "Home" or k == "End" or k == "Enter" or k == "Escape" or k == "Tab" then
			return nil
		end
		-- (A key that edits: whether it erases decides if what's typed is
		-- finished again.)
		erasing = k == "Backspace" or k == "Delete" or (e.cmd and k == "x")
		return nil
	end

	-- The browser setting the field itself (a page's address, Escape's):
	-- not typing.
	local function quiet(text: string)
		ours = text
		erasing = false
	end

	local function on_focus_change(now: boolean)
		focused = now
		if not now then
			-- (A beat later: a row taken with the pointer still lands.)
			ui.after(150, function()
				if not focused then
					close()
				end
			end)
		end
	end

	local function is_open(): boolean
		return out:get() and #shown:get() > 0
	end

	local function descendant(): string?
		local r = shown:get()[active:get()]
		return if is_open() and r then `browser-suggest-{opts.id}-{r.key}` else nil
	end

	local Line = ui.component(function(p: { row: Row, now: State<any> }): Node
		local r, now = p.row, p.now
		local over = ui.use_state(false)
		local function lit(): boolean
			local cur = shown:get()[active:get()]
			return cur ~= nil and cur.key == r.key
		end
		local function it(): Row
			return now:get() :: Row
		end
		local lead: Node
		if r.kind == "search" then
			lead = ui.div { w = 16, h = 16, shrink = 0, items = "center", justify = "center", color = "$text.secondary", Icon { name = "search", size = 14 } }
		elseif r.kind == "preview" then
			lead = ui.div { w = 16, h = 16, shrink = 0, items = "center", justify = "center", color = "$text.secondary", Icon { name = "action-play", size = 14 } }
		else
			lead = Tile {
				size = 16,
				label = function(): string?
					return it().site
				end,
				src = function(): string?
					return it().icon
				end,
			}
		end
		-- The title with what's typed in it lit, and the address beside it
		-- (each fading where it runs out: the title keeps its room first).
		local words = ui.presence(function(): string
			local x = it()
			return `{x.title}\0{x.detail or ""}\0{typed:get()}`
		end, function(): Node
			local x = it()
			local q = typed:peek()
			local title_runs = if x.kind == "search" then { { text = x.title, on = false } } else M.runs(x.title, q)
			local detail: Node? = nil
			if x.detail then
				detail = if x.kind == "visit" or x.kind == "preview"
					then Matched { runs = M.runs(x.detail, q), ink = "$text.tertiary", size = "$font.size.sm", shrink = 2 }
					else ui.text { x.detail, literal = true, shrink = 0, text_size = "$font.size.sm", color = "$text.tertiary" }
			end
			return ui.row {
				grow = 1,
				min_w = 0,
				gap = "$space.2",
				items = "baseline",
				Matched { runs = title_runs, ink = "$text.secondary", size = "$font.size.md", shrink = 1 },
				detail,
			}
		end, { grow = 1, min_w = 0 })
		-- At its end: the dev server's word, a page's × while pointed at, and
		-- ↵ on the row Enter takes.
		local tail = ui.row {
			shrink = 0,
			items = "center",
			gap = "$space.1",
			if r.kind == "preview"
				then ui.text { ui.t("panel.browser.suggest.dev_server"), shrink = 0, text_size = "$font.size.xs", color = "$text.tertiary" }
				else nil,
			if r.visit
				then IconButton {
					icon = "close",
					size = "sm",
					tone = "quiet",
					focusable = false,
					label = ui.t("panel.browser.suggest.remove", { page = r.title }),
					opacity = function(): number
						return if over:get() then 1 else 0
					end,
					on_click = function()
						forget(it())
					end,
				}
				else nil,
			ui.show(function(): boolean
				return lit() and not over:get()
			end, ui.div { aria_hidden = true, ds.Kbd { keys = "enter", tone = "quiet", size = "sm" } }),
		}
		return ui.row {
			id = `browser-suggest-{opts.id}-{r.key}`,
			class = "browser-suggestion",
			role = "option",
			-- (A row keeps its key as what's typed changes: its words follow.)
			label = function(): string
				return it().title
			end,
			selected = lit,
			h = ROW,
			w_full = true,
			pl = "$space.2",
			pr = "$space.1",
			gap = "$space.2",
			items = "center",
			-- (Concentric: the shape's corner less the inset round the rows.)
			rounded = ROW_RADIUS,
			cursor = "pointer",
			-- (Lit from the keys and pointed at wash alike: one row's at a
			-- time, the pointer's lighting it.)
			bg = function(): string
				return if lit() or over:get() then "$hover" else "#00000000"
			end,
			on_hover = function(inside: boolean)
				over:set(inside)
			end,
			on_click = function()
				take(it())
			end,
			lead,
			words,
			tail,
		}
	end)
	local function row(r: Row, _i: number, now: State<any>): Node
		return Line { row = r, now = now }
	end

	-- The list, for the end of the tab's own column (`relative`, `root`):
	-- the field's lower half while it's out. It hangs flush from the field
	-- (`box`), exactly as wide, on the same solid surface and inside the
	-- same edge (the field squares its bottom corners and drops its bottom
	-- edge as it's `joined`), its bottom corners the field's (`radius`), a
	-- hairline inset between the words typed and the rows; it unrolls
	-- downward out of the field and grows or shrinks with its rows. (Drawn
	-- in the tab, over the page, not as a popover: zgx's take the keyboard
	-- from the field unless they follow the caret, zgx-asks/
	-- popover-keep-focus.)
	local function list(box: Ref, root: Ref, radius: number): Node
		local PAD = 4
		local GAP = (ui.theme.get("zeron.list.gap") :: number?) or 2
		local rows = ui.each(shown, row, {
			key = function(r: Row): string
				return r.key
			end,
			direction = "column",
			-- (The token itself, as GAP reads it for the height below.)
			gap = "$zeron.list.gap",
			w_full = true,
		})
		-- Its height: the hairline, the rows and the inset round them, the
		-- bottom edge.
		local tall = ui.computed(function(): number
			local n = #shown:get()
			return 1 + PAD + n * ROW + math.max(0, n - 1) * GAP + PAD + 1
		end)
		local function geometry(): (number, number, number)
			local f, r = box:peek(), root:peek()
			if not f or not r then
				return 0, 0, 0
			end
			return (f.x or 0) - (r.x or 0), (f.y or 0) + (f.height or 0) - (r.y or 0), f.width or 0
		end
		return ui.show(is_open, ui.component(function(): Node
			local left, top, wide = geometry()
			-- Its height: nothing as it mounts, its rows' the moment after (the
			-- unroll), then theirs as they change.
			local unrolled = ui.use_state(false)
			local target = ui.computed(function(): number
				return if unrolled:get() then tall:get() else 0
			end)
			ui.after(0, function()
				unrolled:set(true)
			end)
			return ui.column {
				id = `browser-suggest-{opts.id}`,
				absolute = true,
				left = left,
				top = top,
				w = wide,
				-- (Unrolled from nothing as it opens, then following its rows.)
				h = motion.tween(target, { duration = 160, ease = "outCubic" }),
				overflow = "hidden",
				bg = "$zeron.menu.solid",
				border = 1,
				border_sides = { 0, 1, 1, 1 },
				border_color = "$zeron.field.edge_focus",
				rounded_tl = 0,
				rounded_tr = 0,
				rounded_bl = radius,
				rounded_br = radius,
				shadow = "$zeron.menu.shadow",
				role = "listbox",
				label = ui.t("panel.browser.suggest.label"),
				-- The hairline: inset to the rows' wash, where the field's
				-- words end and the rows begin.
				ui.div { h = 1, shrink = 0, mx = PAD + 8, bg = "$border.subtle", aria_hidden = true },
				ui.column {
					w_full = true,
					shrink = 0,
					p = PAD,
					rows,
				},
			}
		end)({}))
	end

	return {
		keys = keys,
		quiet = quiet,
		on_focus_change = on_focus_change,
		open = is_open,
		descendant = descendant,
		list = list,
		close = close,
		shown = shown,
		active = active,
		typed = typed,
	}
end

return M
