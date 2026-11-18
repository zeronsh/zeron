--!strict
-- Settings → Plugins → Browse: the plugin store, searched on its servers.
-- The page's catalog card (ds.Settings.CatalogCard) with its search, a bar
-- of what narrows it (one category, a keyword, the order), and under it the
-- store's plugins a page at a time, more asked as the page scrolls near the
-- end (`ui.paged`, keyset cursors), in a list that builds only the rows
-- that show. With nothing searched or chosen, a few plugins Zeron
-- features stand over the card.
--
-- Each row is its plugin's as the list changes: a new search keeps showing
-- the rows it had until the new ones arrive (no flash of nothing, no flash
-- of the old), then rows of plugins in both stay the same nodes. A row
-- says what it is and who made it, how many installed it, and whether the
-- device shown has it (Installed) or a newer version waits (Update); it
-- opens the plugin's page (screens/plugins/listing).
--
-- Browsing needs no sign-in, and the device choice at the page's top
-- right stays: what's installed is that device's, and Install goes there.

local ds = require("@ds")
local store = require("@app/screens/plugins/store")
local state = require("@app/screens/plugins/state")
local words = require("@app/screens/plugins/words")
local nav = require("@app/screens/plugins/nav")

local M = {}

-- What's asked: the words searched, a category, a keyword, the order ("":
-- the store's own for what's asked, relevance for words, else popular).
local query = ui.state("store_query", "")
local category = ui.state("store_category", "")
local keyword = ui.state("store_keyword", "")
local sort = ui.state("store_sort", "")
-- Whether Browse has been looked at this run: until then nothing is asked
-- (the page's other tabs never ask the store).
local wanted = ui.state("store_wanted", false)
-- A first page that didn't come: "offline", or the store's words.
local failed = ui.state("store_failed", nil :: string?)
-- How many plugins what's asked matches, as the store counts them (its
-- lists' `total`), from the first page's answer: kept until the next
-- answers, as the rows are; none from a store that doesn't say.
local total = ui.state("store_total", nil :: number?)
local settled = ui.debounced(query, 250)

local function trimmed(s: string): string
	return (string.gsub(s, "^%s*(.-)%s*$", "%1"))
end

-- The order asked for: the person's, else relevance for words searched and
-- popular otherwise. (Relevance means nothing without words.)
local function order(): string
	local s = sort:get()
	local words_asked = trimmed(settled:get()) ~= ""
	if s == "relevance" and not words_asked then
		return "popular"
	end
	if s == "" then
		return if words_asked then "relevance" else "popular"
	end
	return s
end

local function narrowed(): boolean
	return trimmed(query:get()) ~= "" or category:get() ~= "" or keyword:get() ~= ""
end

local function reason(err: any): string
	local m = if type(err) == "table" then err.message else err
	return if type(m) == "string" and m ~= "" then m else "failed"
end

-- The store's plugins, a page at a time. Keyed by what the first page
-- reads (the settled words, the category, the keyword, the order): a
-- change asks again from the start, the rows shown staying until it
-- answers. A later page leaves out any already shown (a plugin updated
-- while paging moves up the "recently updated" order).
local listing: any
listing = ui.paged(function(cursor: string?): any
	if not wanted:get() then
		return { items = {}, next = nil }
	end
	local asked: store.Query = {
		q = trimmed(settled:get()),
		category = if category:get() ~= "" then category:get() else nil,
		keyword = if keyword:get() ~= "" then keyword:get() else nil,
		sort = order(),
		cursor = cursor,
		limit = store.PAGE,
	}
	local ok, page = pcall(function(): any
		return store.search(asked):await()
	end)
	if not ok then
		if cursor == nil then
			failed:set(reason(page))
			total:set(nil)
		end
		error(page, 0)
	end
	if cursor == nil and failed:peek() ~= nil then
		failed:set(nil)
	end
	if cursor == nil then
		local n = if type(page) == "table" and type(page.total) == "number" and page.total >= 0 then math.floor(page.total) else nil
		if total:peek() ~= n then
			total:set(n)
		end
	end
	local items: { store.Card } = {}
	local seen: { [string]: boolean } = {}
	if cursor ~= nil and listing then
		for _, c in listing.items:peek() do
			seen[c.id] = true
		end
	end
	for _, c in (if type(page) == "table" and type(page.items) == "table" then page.items else {}) :: { any } do
		if type(c) == "table" and store.valid_id(c.id) and not seen[c.id] then
			seen[c.id] = true
			table.insert(items, c)
		end
	end
	local next = if type(page) == "table" and type(page.next_cursor) == "string" and page.next_cursor ~= "" then page.next_cursor else nil
	return { items = items, next = next }
end)

-- The few Zeron features, for when nothing narrows the list: the store's
-- featured list (`featured = true`), in its order. (Asked once Browse is
-- looked at.)
local featured_pages = ui.paged(function(_cursor: string?): any
	if not wanted:get() then
		return { items = {}, next = nil }
	end
	local ok, page = pcall(function(): any
		return store.search({ featured = true, sort = "popular", limit = 6 }):await()
	end)
	if not ok or type(page) ~= "table" then
		return { items = {}, next = nil }
	end
	local out: { store.Card } = {}
	for _, c in (if type(page.items) == "table" then page.items else {}) :: { any } do
		-- (The store's featured, in the store's order: the first few.)
		if type(c) == "table" and store.valid_id(c.id) and #out < 3 then
			table.insert(out, c)
		end
	end
	return { items = out, next = nil }
end)

-- The categories the store has, asked once a run.
local categories = ui.state("store_categories", {} :: { { id: string, name: string } })
local asked_categories = false
local function load_categories()
	if asked_categories then
		return
	end
	asked_categories = true
	store.categories():after(function(r: any)
		local list = {}
		for _, c in (if type(r) == "table" and type(r.categories) == "table" then r.categories else {}) :: { any } do
			if type(c) == "table" and type(c.id) == "string" then
				table.insert(list, { id = c.id, name = if type(c.name) == "string" then c.name else c.id })
			end
		end
		categories:set(list)
	end, function()
		-- (Asked again on the next visit.)
		asked_categories = false
	end)
end

-- A category's name, in the locale shown where Zeron knows it.
-- (The store's fixed list, amendment 4: named in each language by id;
-- one the app doesn't know yet goes by the store's own name.)
local KNOWN_CATEGORIES: { [string]: string } = {
	agents = "agents",
	appearance = "appearance",
	chat = "chat",
	["developer-tools"] = "developer_tools",
	git = "git",
	integrations = "integrations",
	productivity = "productivity",
	utilities = "utilities",
}
function M.category_name(id: string): string
	local known = KNOWN_CATEGORIES[id]
	if known then
		return ui.t(`plugins.store.category.{known}`):get()
	end
	for _, c in categories:peek() do
		if c.id == id then
			return c.name
		end
	end
	return id
end

-- What's installed on the device shown from the store, by id (another
-- plugin with the same id, from a file or a link, is never this one).
local installed = ui.computed(function(): { [string]: state.Item }
	local out: { [string]: state.Item } = {}
	for _, p in state.shown:get() do
		local source: any = p.source
		if type(source) == "table" and source.kind == "store" then
			out[p.id] = p
		end
	end
	return out
end)

-- Whether the device shown has it, and whether a newer version waits for
-- it there: "update", "installed", or "". (The update is the engine's to
-- find, as for any installed plugin: it alone knows which version that
-- device's Zeron takes, and whether it takes pre-releases.)
function M.standing(card: store.Card): string
	local it = installed:get()[card.id]
	if not it then
		return ""
	end
	if type(it.update) == "table" and it.update.refused == nil then
		return "update"
	end
	return "installed"
end

-- Who made it, as every plugin says it: by @username (publishing needs
-- one, so every listing has its account's).
function M.author(card: any): string
	local a = card.author or {}
	if type(a.username) == "string" and a.username ~= "" then
		return words.author({ author = { kind = "verified", username = a.username } })
	end
	return ui.t("plugins.words.author.unknown"):get()
end

-- "by @ada · 1,204 installs".
function M.byline(card: any): string
	local n = if type(card.installs) == "number" then math.max(0, math.floor(card.installs)) else 0
	return ui.t("plugins.store.byline", { author = M.author(card), n = n }):get()
end

-- A plugin's tile: its icon from the store (an SVG of its package, as an
-- installed plugin's), else its initial on its hue, as installed rows draw.
function M.Tile(props: { card: State<store.Card> | () -> store.Card, size: number }): Node
	local card = props.card
	local function now(): store.Card
		return if type(card) == "function" then (card :: () -> store.Card)() else (card :: State<store.Card>):get()
	end
	return ui.presence(function(): string
		return if store.icon_svg(now()) then "icon" else "initial"
	end, function(kind: string): Node
		local c = now()
		if kind == "icon" then
			return words.tile({ id = c.id, name = c.name, icon_svg = store.icon_svg(c) }, props.size)
		end
		return words.tile({ id = c.id, name = c.name }, props.size)
	end)
end

-- Its account's badges (what Zeron's operators granted it, as its member
-- card shows them), beside its name: each its name, quiet.
function M.badges(card: () -> store.Card): Node
	local function names(): { string }
		local out: { string } = {}
		local a: any = card().author
		for _, b in (if type(a) == "table" and type(a.badges) == "table" then a.badges else {}) :: { any } do
			local name = if type(b) == "table" then b.name else b
			if type(name) == "string" and name ~= "" and #out < 3 then
				table.insert(out, string.sub(name, 1, 40))
			end
		end
		return out
	end
	local list = ui.computed(names)
	return ui.row {
		gap = "$space.1",
		items = "center",
		ui.each(list, function(name: string): Node
			return ds.Badge {
				function(): string
					return name
				end,
				-- (The app's badge: ink on its wash, as Devices' "This device".)
				tone = "neutral",
				size = "sm",
			}
		end, {
			key = function(name: string): string
				return name
			end,
		}),
	}
end

-- A row's height: a truncated row with a byline over its line.
local function row_h(_c: store.Card): number
	return ds.Settings.truncated_row_h(32, true)
end

local function row(first: store.Card, _i: State<number>, now: State<store.Card>): Node
	local function card(): store.Card
		return now:get()
	end
	return ds.Settings.Row {
		-- (Its words drawn as they are: a store's are never a template.)
		label = function(): string
			return now:get().name
		end,
		badge = M.badges(card),
		byline = function(): string
			return M.byline(now:get())
		end,
		description = function(): string
			return now:get().summary or ""
		end,
		lead = M.Tile { card = now, size = 32 },
		truncate = true,
		value = function(): string
			local s = M.standing(now:get())
			if s == "update" then
				return ui.t("plugins.store.update"):get()
			elseif s == "installed" then
				return ui.t("plugins.store.installed"):get()
			end
			return ""
		end,
		on_click = function()
			nav.open_listing(now:peek().id)
		end,
	}
end

-- The store's categories, the card's row under its search: the app's quiet
-- tabs (All first) across the card's width, their first's words on the
-- rows' text edge. Too many for the room (a narrow pane, a longer
-- language), they run sideways and the edge hiding some shows a chevron
-- that pages them (ds/pager). One Tab stop, the arrows between them.
local function category_tabs(): Node
	local tabs = ui.presence(function(): string
		return if #categories:get() > 0 then "ready" else "waiting"
	end, function(key: string): Node
		-- (The store's list arrives once: built then, the tabs its own.)
		local options: { any } = { { value = "", label = ui.t("plugins.store.all") } }
		if key == "ready" then
			for _, c in categories:peek() do
				local name = M.category_name(c.id)
				table.insert(options, { value = c.id, label = name })
			end
		end
		return ds.Tabs {
			value = category,
			label = ui.t("plugins.store.category_label"),
			-- (The app's tabs, underlined, as Settings' and a diff's are; the
			-- card's own rule under them is their line.)
			rule = false,
			options = options,
		}
	end)
	-- (A tab's words are 4 inside it, so 12 here puts the first's on the
	-- rows' 16; 12 at the end as well. Its bar sits on the card's rule.)
	return ui.row {
		id = "store-filters",
		w_full = true,
		px = "$space.3",
		ds.Pager {
			id = "store-categories",
			back = ui.t("plugins.store.categories_back"),
			more = ui.t("plugins.store.categories_more"),
			tabs,
		},
	}
end

-- The order the list is in: the list's section's own control at its
-- title's end, saying the order and opening the choice.
local function order_control(): Node
	local function sort_items(): { any }
		local out: { any } = {}
		if trimmed(query:get()) ~= "" then
			table.insert(out, { value = "relevance", label = ui.t("plugins.store.sort.relevance"):get() })
		end
		table.insert(out, { value = "popular", label = ui.t("plugins.store.sort.popular"):get() })
		table.insert(out, { value = "updated", label = ui.t("plugins.store.sort.updated"):get() })
		table.insert(out, { value = "new", label = ui.t("plugins.store.sort.new"):get() })
		return out
	end
	-- The order shown: the one in use (relevance only with words).
	local shown_sort = ui.computed(function(): string
		local s = sort:get()
		local w = trimmed(query:get()) ~= ""
		if s == "" or (s == "relevance" and not w) then
			return if w then "relevance" else "popular"
		end
		return s
	end)
	local sort_value = ui.state("store_sort_shown", shown_sort:peek())
	ui.effect(function()
		local s = shown_sort:get()
		if sort_value:peek() ~= s then
			sort_value:set(s)
		end
	end)
	ui.effect(function()
		local v = sort_value:get()
		if v ~= shown_sort:peek() then
			sort:set(v)
		end
	end)
	-- (The app's choice of a few words: a Select, small, as Settings'
	-- smaller choices are.)
	return ui.div {
		id = "store-order",
		ds.Select {
			value = sort_value :: any,
			options = sort_items,
			size = "sm",
			-- (As Settings' small choices: wide enough for the longest,
			-- "Recently updated".)
			w = 160,
			name = ui.t("plugins.store.order_label"),
		},
	}
end

-- Clears what narrows it: words, category and keyword.
local function clear()
	query:set("")
	category:set("")
	keyword:set("")
end

local function said(node: Node): Node
	return ui.column { w_full = true, py = "$space.6", px = "$space.4", node }
end

-- What the card holds under its bar, by kind.
local function inside(key: string): Node
	if key == "loading" then
		return ds.SkeletonWaiting {
			build = function(still: boolean): Node
				return ui.column {
					id = if still then "store-loading-still" else "store-loading",
					w_full = true,
					ds.SkeletonRow { title = 120, byline = 90, line = 260, trail = 0, still = still },
					ds.Settings.Rule(),
					ds.SkeletonRow { title = 96, byline = 110, line = 220, trail = 0, still = still },
					ds.Settings.Rule(),
					ds.SkeletonRow { title = 140, byline = 80, line = 240, trail = 0, still = still },
					ds.Settings.Rule(),
					ds.SkeletonRow { title = 84, byline = 100, line = 200, trail = 0, still = still },
				}
			end,
		}
	elseif key == "offline" then
		return said(ds.EmptyState {
			icon = "wifi-off",
			title = ui.t("plugins.store.offline_title"),
			body = ui.t("plugins.store.offline_body"),
			size = "sm",
			action = { label = ui.t("plugins.store.try_again"), on_click = listing.reload },
		})
	elseif key == "failed" then
		return said(ds.EmptyState {
			icon = "danger-triangle",
			title = ui.t("plugins.store.failed_title"),
			body = function(): string
				return words.failure(failed:get() or "")
			end,
			size = "sm",
			action = { label = ui.t("plugins.store.try_again"), on_click = listing.reload },
		})
	elseif key == "nomatch" then
		return said(ds.EmptyState {
			icon = "search",
			title = function(): string
				local q = trimmed(query:get())
				return if q ~= ""
					then ui.t("plugins.store.nothing_matches", { query = q }):get()
					else ui.t("plugins.store.nothing_here"):get()
			end,
			body = ui.t("plugins.store.try_another"),
			size = "sm",
			action = { label = ui.t("plugins.store.clear"), on_click = clear },
		})
	elseif key == "none" then
		return said(ds.EmptyState {
			icon = "widget",
			title = ui.t("plugins.store.empty_title"),
			body = ui.t("plugins.store.empty_body"),
			size = "sm",
		})
	end
	-- The rows, and at their foot the next page coming (or why it didn't).
	return ui.column {
		id = "store-rows",
		w_full = true,
		ds.Settings.ListRows(listing.items, row, {
			key = function(c: store.Card): string
				return c.id
			end,
			height = row_h,
			text = function(c: store.Card): string
				return c.name
			end,
		}),
		ui.show(function(): boolean
			return listing.loading:get() and listing.status:get() == "ready"
		end, ui.column {
			id = "store-more",
			w_full = true,
			ds.Settings.Rule(),
			ds.SkeletonWaiting {
				build = function(still: boolean): Node
					return ui.div {
						id = if still then "store-more-still" else nil,
						w_full = true,
						ds.SkeletonRow { title = 110, byline = 90, line = 240, trail = 0, still = still },
					}
				end,
			},
		}),
		ui.show(function(): boolean
			return listing.status:get() == "failed" and failed:get() == nil
		end, ui.column {
			w_full = true,
			ds.Settings.Rule(),
			ui.row {
				w_full = true,
				px = "$space.4",
				py = "$space.3",
				gap = "$space.3",
				items = "center",
				ui.text { ui.t("plugins.store.more_failed"), grow = 1, min_w = 0, wrap = true, text_size = "$font.size.sm", line_height = "$font.line.sm", color = "$text.tertiary" },
				ds.Button { ui.t("plugins.store.try_again"), tone = "secondary", size = "sm", on_click = listing.more },
			},
		}),
		-- Where the list ends: coming near the view, the next page is
		-- asked. A mark of its own for each end the list has had, so its
		-- first layout says whether the end a page made is still near (a
		-- view the rows don't fill yet asks again).
		ui.presence(function(): string
			return tostring(#listing.items:get())
		end, function(_: string): Node
			return ui.div {
				id = "store-end",
				w_full = true,
				h = 1,
				visible_margin = 600,
				on_visible = function(visible: boolean)
					if visible and not listing.loading:peek() and not listing.done:peek() and listing.status:peek() == "ready" then
						listing.more()
					end
				end,
			}
		end),
	}
end

local function body_key(): string
	local items = #listing.items:get()
	local s = listing.status:get()
	if items > 0 and not (s == "failed" and failed:get() ~= nil) then
		return "list"
	end
	if s == "failed" then
		local word = string.match(failed:get() or "", "^([%l_]+)")
		return if word == "offline" or word == "store_unreachable" then "offline" else "failed"
	end
	if s == "loading" or listing.loading:get() or not wanted:get() then
		return "loading"
	end
	return if narrowed() then "nomatch" else "none"
end

-- One featured plugin, a card of its own that opens it.
local function feature(first: store.Card, _i: number, now: State<store.Card>): Node
	return ds.Settings.Feature {
		label = function(): string
			return now:get().name
		end,
		byline = function(): string
			return M.author(now:get())
		end,
		description = function(): string
			return now:get().summary or ""
		end,
		-- (A list row's head: its tile at a row's size.)
		lead = M.Tile { card = now, size = 32 },
		on_click = function()
			nav.open_listing(now:peek().id)
		end,
	}
end

-- The tab: the featured over the card, the card's search, bar and rows.
M.View = ui.component(function(): Node
	-- (As it shows, never as it builds: a reload builds it again.)
	ui.on_mount(function()
		if not wanted:peek() then
			wanted:set(true)
		end
		load_categories()
	end)
	-- The featured: while they come, their room held by cards of their
	-- shape (nothing under them moves when they arrive); none, no room.
	local function featured_key(): string
		if narrowed() then
			return "none"
		elseif #featured_pages.items:get() > 0 then
			return "cards"
		elseif featured_pages.status:get() == "loading" or not wanted:get() then
			return "loading"
		end
		return "none"
	end
	return ui.column {
		w_full = true,
		-- (Sections apart as a Settings page's are.)
		gap = "$space.6",
		ui.show(function(): boolean
			return featured_key() ~= "none"
		end, ui.presence(featured_key, function(key: string): Node
			-- (One shelf, the picture strip's rule: three share the column
			-- while each keeps its least; under that they run sideways, the
			-- next cut by the column's end. Never two and one.)
			return ds.Settings.Section {
				title = ui.t("plugins.store.featured"),
				if key == "loading"
					then ds.SkeletonWaiting {
						build = function(still: boolean): Node
							return ds.Shelf {
								id = "store-featured-loading",
								across = 3,
								min_w = ds.Settings.FEATURE_MIN,
								count = function(): number
									return 3
								end,
								build = function(w: () -> number): Node
									local cards: { Node } = {}
									for _ = 1, 3 do
										table.insert(cards, ui.div { w = w, shrink = 0, ds.Settings.FeatureSkeleton { still = still } })
									end
									return cards :: any
								end,
							}
						end,
					}
					else ds.Shelf {
						id = "store-featured",
						across = 3,
						min_w = ds.Settings.FEATURE_MIN,
						count = function(): number
							return #featured_pages.items:get()
						end,
						build = function(w: () -> number): Node
							return ui.each(featured_pages.items, function(first: store.Card, i: number, now: State<store.Card>): Node
								return ui.div { w = w, shrink = 0, feature(first, i, now) }
							end, {
								key = function(c: store.Card): string
									return c.id
								end,
							})
						end,
					},
			}
		end, { w_full = true })),
		-- The store's plugins: a section, its title how many what's asked
		-- matches (the store's `total`; "Plugins" while it isn't known, or
		-- from a store that doesn't count), its action the order; then the
		-- card: the search, the categories, the rows. (Its head is one
		-- height loading or loaded: nothing under it moves.)
		ui.column { id = "store-list", w_full = true, ds.Settings.Section {
			title = function(): string
				local n = total:get()
				return if n and n > 0 then ui.t("plugins.store.count", { n = n }):get() else ui.t("plugins.store.list_title"):get()
			end,
			trail = order_control(),
			ds.Settings.CatalogCard {
				id = "store-card",
				search = { value = query, placeholder = ui.t("plugins.store.search") },
				-- The keyword a plugin's page narrowed it to: one token, removable.
				search_trail = ui.show(function(): boolean
					return keyword:get() ~= ""
				end, ds.Tag {
					function(): string
						return keyword:get()
					end :: any,
					on_remove = function()
						keyword:set("")
					end,
				}),
				filters = category_tabs(),
				body = ui.presence(body_key, inside, { w_full = true, enter = { opacity = { 0, 1 }, duration = 120 }, exit = { opacity = 0, duration = 60 }, mode = "wait" }),
			},
		} },
	}
end)

-- For the page's link from elsewhere (a keyword on a plugin's page):
-- Browse narrowed to it.
function M.show_keyword(k: string)
	query:set("")
	category:set("")
	keyword:set(k)
end

-- (Tests.)
M._listing = listing

return M
