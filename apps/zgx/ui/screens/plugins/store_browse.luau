--!strict
-- Settings → Plugins → Browse: the plugin store, searched on its servers.
-- The list's head says how many and holds what narrows and orders it (a
-- category, the order); the page's catalog card (ds.Settings.CatalogCard)
-- holds its search (a keyword beside it) and under it the store's plugins
-- a page at a time, more asked as the page scrolls near the
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
local words = require("@app/screens/plugins/words")
local client = require("@app/screens/plugins/store_client")
local kinds = require("@app/screens/plugins/store_kind")

local C = {}

function C.make(kind: kinds.Kind)
	local store, nav, state = kind.store, kind.nav, kind.state
	local say = kinds.say(kind)
	local M = {}

	-- What's asked: the words searched, a category, a keyword, a publisher
	-- (a username, from a plugin's page), whether verified plugins only, the
	-- order ("": the store's own for what's asked, relevance for words, else
	-- popular).
	local query = ui.state(kind.keys .. "_query", "")
	local category = ui.state(kind.keys .. "_category", "")
	local keyword = ui.state(kind.keys .. "_keyword", "")
	local author = ui.state(kind.keys .. "_author", "")
	local verified_only = ui.state(kind.keys .. "_verified_only", false)
	local sort = ui.state(kind.keys .. "_sort", "")
	-- Whether Browse has been looked at this run: until then nothing is asked
	-- (the page's other tabs never ask the store).
	local wanted = ui.state(kind.keys .. "_wanted", false)
	-- A first page that didn't come: "offline", or the store's words.
	local failed = ui.state(kind.keys .. "_failed", nil :: string?)
	-- How many plugins what's asked matches, as the store counts them (its
	-- lists' `total`), from the first page's answer: kept until the next
	-- answers, as the rows are; none from a store that doesn't say.
	-- Whether the first page has more after it.
	local paged = ui.state(kind.keys .. "_paged", false)
	local total = ui.state(kind.keys .. "_total", nil :: number?)
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
		return trimmed(query:get()) ~= "" or category:get() ~= "" or keyword:get() ~= "" or author:get() ~= "" or verified_only:get()
	end

	local function reason(err: any): string
		local m = if type(err) == "table" then err.message else err
		return if type(m) == "string" and m ~= "" then m else "failed"
	end

	-- The store's plugins, a page at a time. Keyed by what the first page
	-- reads (the settled words, the category, the keyword, the publisher,
	-- verified only, the order): a
	-- change asks again from the start, the rows shown staying until it
	-- answers. A later page leaves out any already shown (a plugin updated
	-- while paging moves up the "recently updated" order).
	local listing: any
	listing = ui.paged(function(cursor: string?): any
		if not wanted:get() then
			return { items = {}, next = nil }
		end
		local asked: client.Query = {
			q = trimmed(settled:get()),
			category = if category:get() ~= "" then category:get() else nil,
			keyword = if keyword:get() ~= "" then keyword:get() else nil,
			author = if author:get() ~= "" then author:get() else nil,
			verified = if verified_only:get() then true else nil,
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
		local items: { client.Card } = {}
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
		if cursor == nil and paged:peek() ~= (next ~= nil) then
			paged:set(next ~= nil)
		end
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
		local out: { client.Card } = {}
		for _, c in (if type(page.items) == "table" then page.items else {}) :: { any } do
			-- (The store's featured, in the store's order: the first few.)
			if type(c) == "table" and store.valid_id(c.id) and #out < 3 then
				table.insert(out, c)
			end
		end
		return { items = out, next = nil }
	end)

	-- Whether the featured stand over the list: with nothing narrowing it,
	-- and (cards, where they'd be the list's own first again) only over a
	-- list of more than a page.
	local function features(): boolean
		return not narrowed() and #featured_pages.items:get() > 0 and (not kind.cards or paged:get())
	end
	-- The list as it's drawn (cards): without what's featured over it.
	local drawn = ui.computed(function(): { client.Card }
		local all = listing.items:get()
		if not kind.cards or not features() then
			return all
		end
		local over: { [string]: boolean } = {}
		for _, c in featured_pages.items:get() do
			over[c.id] = true
		end
		local out: { client.Card } = {}
		for _, c in all do
			if not over[c.id] then
				table.insert(out, c)
			end
		end
		return out
	end)

	-- The categories the store has, asked once a run, each with how many
	-- listed plugins it holds when the store says.
	local categories = ui.state(kind.keys .. "_categories", {} :: { { id: string, name: string, count: number? } })
	local asked_categories = false
	local function load_categories()
		if asked_categories or not kind.categories then
			return
		end
		asked_categories = true
		store.categories():after(function(r: any)
			local list = {}
			for _, c in (if type(r) == "table" and type(r.categories) == "table" then r.categories else {}) :: { any } do
				if type(c) == "table" and type(c.id) == "string" then
					table.insert(list, {
						id = c.id,
						name = if type(c.name) == "string" then c.name else c.id,
						count = if type(c.count) == "number" and c.count >= 0 then math.floor(c.count) else nil,
					})
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
	local installed = ui.computed(function(): { [string]: any }
		local out: { [string]: any } = {}
		for _, p in state.shown:get() do
			local source: any = p.source
			if type(source) == "table" and source.kind == "store" then
				-- (By the store's id: a template's folder may be named apart.)
				out[source.id or p.id] = p
			end
		end
		return out
	end)

	-- Whether the device shown has it, and whether a newer version waits for
	-- it there: "update", "installed", or "". (The update is the engine's to
	-- find, as for any installed plugin: it alone knows which version that
	-- device's Zeron takes, and whether it takes pre-releases.)
	function M.standing(card: client.Card): string
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
	function M.Tile(props: { card: State<client.Card> | () -> client.Card, size: number }): Node
		local card = props.card
		local function now(): client.Card
			return if type(card) == "function" then (card :: () -> client.Card)() else (card :: State<client.Card>):get()
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

	-- The store's mark beside its name: "Verified" when the store has verified
	-- the version the card offers (what the store's answer says, for this
	-- visit: the catalog's, never a sheet's decision). Nil when it hasn't.
	-- (A template's says it isn't, in words: its author draws the picture over
	-- it, and may draw a mark.)
	function M.verified_mark(card: () -> client.Card): Node
		local verified = ui.show(function(): boolean
			return card().verified == true
		end, ds.Badge { ui.t("plugins.store.verified"), class = "store-verified", tone = "success", size = "sm" })
		if not kind.cards then
			return verified
		end
		return ui.row {
			items = "center",
			verified,
			ui.show(function(): boolean
				return card().verified ~= true
			end, ds.Badge { ui.t("plugins.store.review.waiting"), class = "store-not-verified", tone = "warning", size = "sm" }),
		}
	end

	-- A version's state as the store keeps it (listed, yanked, taken down,
	-- removed), its review apart: the store verifies; it doesn't gate. (An
	-- older answer says `yanked` alone.)
	function M.version_state(v: any): string
		if type(v.status) == "string" and v.status ~= "" then
			return v.status
		end
		return if v.yanked == true then "yanked" else "listed"
	end

	-- Where its review stands: "verified"; "declined" in its author's view
	-- alone (`owner`: told why), for the public never learns of a decline;
	-- else "waiting".
	function M.version_review(v: any, owner: boolean?): string
		if v.verified == true then
			return "verified"
		end
		local r = v.review
		return if owner and type(r) == "table" and r.decision == "declined" then "declined" else "waiting"
	end

	-- A version's badge beside its number, in every list of versions (a
	-- plugin's page, Publish's): its state's when it takes no new installs,
	-- was taken down or removed; else its review's ("Verified", "Not verified
	-- yet", its author's "Not verified"), so the eye runs down the numbers and
	-- reads which are verified.
	local STATE_TONE: { [string]: string } = {
		yanked = "info",
		taken_down = "warning",
		removed = "neutral",
	}
	local REVIEW_TONE: { [string]: string } = {
		waiting = "warning",
		declined = "danger",
	}
	function M.version_badge(version: () -> any, owner: boolean?): Node
		return ui.switch(function(): string
			local v = version()
			local s = M.version_state(v)
			return if STATE_TONE[s] then s else M.version_review(v, owner)
		end, (function(): { [string]: Node }
			local out: { [string]: Node } = {}
			for s, tone in STATE_TONE do
				out[s] = ds.Badge { ui.t(`plugins.store.status.{s}`), tone = tone :: any, size = "sm" }
			end
			for r, tone in REVIEW_TONE do
				out[r] = ds.Badge { ui.t(`plugins.store.review.{r}`), tone = tone :: any, size = "sm" }
			end
			out.verified = ds.Badge { ui.t("plugins.store.verified"), class = "store-verified", tone = "success", size = "sm" }
			return out
		end)())
	end

	-- Its account's badges (what Zeron's operators granted it, as its member
	-- card shows them): their names, at most three, each once.
	local function account_badges(card: client.Card): { string }
		local out: { string } = {}
		local a: any = card.author
		for _, b in (if type(a) == "table" and type(a.badges) == "table" then a.badges else {}) :: { any } do
			local name = if type(b) == "table" then b.name else b
			if type(name) == "string" and name ~= "" and #out < 3 then
				name = string.sub(name, 1, 40)
				if not table.find(out, name) then
					table.insert(out, name)
				end
			end
		end
		return out
	end

	-- An account's badge: the app's (ink on its wash, as Devices' "This
	-- device"), its words drawn as they are (a store's are never a template).
	local function account_badge(name: string): Node
		return ds.Badge {
			function(): string
				return name
			end,
			tone = "neutral",
			size = "sm",
		}
	end

	-- Its picture, by the store's path for it: its wash while it comes, and
	-- where it has none.
	function M.cover(card: () -> client.Card): Node
		return ui.presence(function(): string
			local c = card()
			local path = (c :: any).cover
			local src = if type(path) == "string" and path ~= "" then store.picture_src(c.id, c.version, path) else false
			return if type(src) == "string" then src else ""
		end, function(src: string): Node
			if src == "" then
				return ui.div { w_full = true, h_full = true }
			end
			return ui.image { src = src, w_full = true, h_full = true, fit = "cover", aria_hidden = true }
		end, { w_full = true, h_full = true })
	end

	-- Whether it's here: "Installed", or that a newer version waits.
	local function here_badge(card: () -> client.Card): Node
		return ui.switch(function(): string
			return M.standing(card())
		end, {
			[""] = ui.div {},
			installed = ds.Badge { ui.t("plugins.store.installed"), tone = "neutral", size = "sm" },
			update = ds.Badge { ui.t("plugins.store.update"), tone = "accent", size = "sm" },
		})
	end

	-- Its badges beside its name on its page: the store's mark first, then
	-- its account's, each its name, quiet (wrapping with who made it).
	function M.badges(card: () -> client.Card): Node
		local list = ui.computed(function(): { string }
			return account_badges(card())
		end)
		return ui.row {
			gap = "$space.1",
			items = "center",
			M.verified_mark(card),
			ui.each(list, account_badge, {
				key = function(name: string): string
					return name
				end,
			}),
		}
	end

	-- The same on a list's row, each whole or absent (ds/settings `badges`):
	-- in a narrow row the account's go first, then the mark, never cut.
	function M.row_badges(card: () -> client.Card): ds.RowBadges
		return {
			keys = function(): { string }
				local out: { string } = {}
				local c = card()
				if c.verified == true then
					table.insert(out, "verified")
				end
				for _, name in account_badges(c) do
					table.insert(out, `account:{name}`)
				end
				return out
			end,
			badge = function(key: string): Node
				if key == "verified" then
					return ds.Badge { ui.t("plugins.store.verified"), class = "store-verified", tone = "success", size = "sm" }
				end
				return account_badge(string.sub(key, #"account:" + 1))
			end,
		}
	end

	-- The least a card of a picture is across.
	local CARD_MIN = 240

	-- A row's height: a truncated row with a byline over its line.
	local function row_h(_c: client.Card): number
		return ds.Settings.truncated_row_h(32, true)
	end

	local function row(first: client.Card, _i: State<number>, now: State<client.Card>): Node
		local function card(): client.Card
			return now:get()
		end
		return ds.Settings.Row {
			-- (Its words drawn as they are: a store's are never a template.)
			label = function(): string
				return now:get().name
			end,
			badges = M.row_badges(card),
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

	-- The list head's Selects' width (the app's small Select, as Settings'
	-- smaller choices): enough for the longest, "Developer tools" and
	-- "Recently updated".
	local SELECT_W = 160

	-- The store's categories: the app's small Select in the list's head beside
	-- Order, "All categories" first (nine of them don't fit across a pane as
	-- tabs, and a row that scrolls in every window is worse than a choice).
	local function category_control(): Node
		local function options(): { any }
			local out: { any } = { { value = "", label = ui.t("plugins.store.all_categories"):get() } }
			for _, c in categories:get() do
				local name = M.category_name(c.id)
				table.insert(out, {
					value = c.id,
					-- (How many listed plugins it holds, when the store says.)
					label = if type(c.count) == "number" then ui.t("plugins.store.category_count", { name = name, n = c.count }):get() else name,
				})
			end
			return out
		end
		return ui.div {
			id = "store-category",
			ds.Select {
				value = category,
				options = options,
				size = "sm",
				w = SELECT_W,
				name = ui.t("plugins.store.category_label"),
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
		local sort_value = ui.state(kind.keys .. "_sort_shown", shown_sort:peek())
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
				w = SELECT_W,
				name = ui.t("plugins.store.order_label"),
			},
		}
	end

	-- The list's head: its count as the title at the left, and at the right
	-- what narrows and orders it, in this order: Verified only, Category,
	-- Order. Where the pane can't hold
	-- the title and them on one line, they go on a line of their own under the
	-- title, ending on the content edge as they do beside it.
	local list_w = ui.state(kind.keys .. "_list_w", 0)
	-- (Verified only's own width, as its words make it: the Selects' are fixed.)
	local check_w = ui.state(kind.keys .. "_check_w", 0)
	-- (The least the count's words keep beside them: "1,234 plugins".)
	local TITLE_LEAST = 120
	local function gap(): number
		return (ui.theme.get("space.2") :: number?) or 8
	end
	-- The controls' width side by side, from what they are (never from where
	-- they're placed, which follows from it).
	local function controls_w(): number
		local selects = if kind.categories then 2 else 1
		return check_w:get() + selects * SELECT_W + selects * gap()
	end
	local function head_on_one_line(): boolean
		local w, c = list_w:get(), check_w:get()
		-- (Not yet measured: on one line, as a wide pane has it.)
		if w <= 0 or c <= 0 then
			return true
		end
		return w - 2 * ds.Settings.TEXT_INSET >= controls_w() + ((ui.theme.get("space.3") :: number?) or 12) + TITLE_LEAST
	end
	-- `wrap`: on a line of its own, Verified only going over the Selects (which
	-- stay together) where even that line can't hold them, all ending on the
	-- content edge.
	local function head_controls(wrap: boolean): Node
		return ui.row {
			id = "store-head-controls",
			w_full = wrap,
			flex_wrap = wrap,
			justify = "end",
			gap = "$space.2",
			items = "center",
			-- (The app's checkbox, its words its own: a filter that is on or off.)
			ui.div {
				id = "store-verified-only",
				shrink = 0,
				on_resize = function(size: { w: number, h: number })
					if math.abs(size.w - check_w:peek()) >= 0.5 then
						check_w:set(size.w)
					end
				end,
				ds.Checkbox { value = verified_only, label = ui.t("plugins.store.verified_only") },
			},
			-- (The two Selects one unit: they never part on a wrap.)
			ui.row { shrink = 0, gap = "$space.2", items = "center", if kind.categories then category_control() else nil, order_control() },
		}
	end

	-- Clears what narrows it: words, category and keyword.
	local function clear()
		query:set("")
		category:set("")
		keyword:set("")
		author:set("")
		verified_only:set(false)
	end

	local function said(node: Node): Node
		return ui.column { w_full = true, py = "$space.6", px = "$space.4", node }
	end

	-- A card of one, its picture first: opens its page.
	-- (Featured, narrower: who made it alone, as a plugin's says.)
	local function card_of(now: State<client.Card>, featured: boolean?): Node
		local function card(): client.Card
			return now:get()
		end
		return ds.Settings.Feature {
			label = function(): string
				return card().name
			end,
			byline = function(): string
				return if featured then M.author(card()) else M.byline(card())
			end,
			description = function(): string
				return card().summary or ""
			end,
			badge = ui.row { gap = "$space.1", items = "center", M.verified_mark(card), here_badge(card) },
			cover = M.cover(card),
			on_click = function()
				nav.open_listing(now:peek().id)
			end,
		}
	end

	-- What the card holds under its bar, by kind.
	local function inside(key: string): Node
		if key == "loading" then
			return ds.SkeletonWaiting {
				build = function(still: boolean): Node
					if kind.cards then
						return ds.Settings.Cards {
							id = if still then "store-loading-still" else "store-loading",
							min_w = CARD_MIN,
							build = function(w: () -> number): Node
								local cards: { Node } = {}
								for _ = 1, 4 do
									table.insert(cards, ui.div { w = w, shrink = 0, ds.Settings.FeatureSkeleton { still = still, cover = true } })
								end
								return ui.row { w_full = true, flex_wrap = true, gap = "$space.3", table.unpack(cards) }
							end,
						}
					end
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
				title = say("plugins.store.offline_title"),
				body = say("plugins.store.offline_body"),
				size = "sm",
				action = { label = ui.t("plugins.store.try_again"), on_click = listing.reload },
			})
		elseif key == "failed" then
			return said(ds.EmptyState {
				icon = "danger-triangle",
				title = say("plugins.store.failed_title"),
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
						else say("plugins.store.nothing_here"):get()
				end,
				body = ui.t("plugins.store.try_another"),
				size = "sm",
				action = { label = ui.t("plugins.store.clear"), on_click = clear },
			})
		elseif key == "none" then
			return said(ds.EmptyState {
				icon = "widget",
				title = say("plugins.store.empty_title"),
				body = say("plugins.store.empty_body"),
				size = "sm",
			})
		end
		-- The rows, and at their foot the next page coming (or why it didn't).
		return ui.column {
			id = "store-rows",
			w_full = true,
			if kind.cards
				then ds.Settings.Cards {
					id = "store-cards",
					min_w = CARD_MIN,
					build = function(w: () -> number): Node
						return ui.each(drawn, function(_first: client.Card, _i: number, now: State<client.Card>): Node
							return ui.div { w = w, shrink = 0, card_of(now) }
						end, {
							key = function(c: client.Card): string
								return c.id
							end,
							direction = "row",
							flex_wrap = true,
							w_full = true,
							gap = "$space.3",
						})
					end,
				}
				else ds.Settings.ListRows(listing.items, row, {
					key = function(c: client.Card): string
						return c.id
					end,
					height = row_h,
					text = function(c: client.Card): string
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
					ui.text { say("plugins.store.more_failed"), grow = 1, min_w = 0, wrap = true, text_size = "$font.size.sm", line_height = "$font.line.sm", color = "$text.tertiary" },
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
		-- (Cards wait for what's featured over them too, so nothing moves as
		-- it comes.)
		if kind.cards and not narrowed() and featured_pages.status:get() == "loading" then
			return "loading"
		end
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
	local function feature(first: client.Card, _i: number, now: State<client.Card>): Node
		if kind.cards then
			return card_of(now, true)
		end
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
			-- (The mark as everywhere, on the line under the name, with who
			-- made it: the name has its line to itself.)
			badge = M.verified_mark(function(): client.Card
				return now:get()
			end),
			on_click = function()
				nav.open_listing(now:peek().id)
			end,
		}
	end

	-- Under the list's head: the search, and (cards,
	-- which stand on the page) the search's own card apart from them.
	local function list_body(): { Node }
		local out: { Node } = {}
		table.insert(out, ds.Settings.CatalogCard {
			id = "store-card",
			search = { value = query, placeholder = say("plugins.store.search") },
			-- What a plugin's page narrowed it to (its keyword, its
			-- publisher): a token each, removable.
			search_trail = ui.show(function(): boolean
				return keyword:get() ~= "" or author:get() ~= ""
			end, ui.row {
				gap = "$space.1",
				items = "center",
				ui.show(function(): boolean
					return keyword:get() ~= ""
				end, ds.Tag {
					function(): string
						return keyword:get()
					end :: any,
					on_remove = function()
						keyword:set("")
					end,
				}),
				ui.show(function(): boolean
					return author:get() ~= ""
				end, ds.Tag {
					function(): string
						return `@{author:get()}`
					end :: any,
					on_remove = function()
						author:set("")
					end,
				}),
			}),
			body = if kind.cards then nil else ui.presence(body_key, inside, { w_full = true, enter = { opacity = { 0, 1 }, duration = 120 }, exit = { opacity = 0, duration = 60 }, mode = "wait" }),
		})
		if kind.cards then
			-- (The grid's own gap under the search.)
			table.insert(out, ui.column { w_full = true, pt = "$space.1", ui.presence(body_key, inside, { w_full = true, enter = { opacity = { 0, 1 }, duration = 120 }, exit = { opacity = 0, duration = 60 }, mode = "wait" }) })
		end
		return out
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
			elseif features() then
				return "cards"
			elseif kind.cards then
				return "none"
			elseif featured_pages.status:get() == "loading" or not wanted:get() then
				return "loading"
			end
			return "none"
		end
		return ui.column {
			w_full = true,
			-- (Sections apart as a Settings page's are.)
			gap = "$space.6",
			-- (What's said of all of it, once, under the tabs.)
			if kind.fact
				then ui.text {
					id = "store-fact",
					ui.t(kind.fact),
					w_full = true,
					px = ds.Settings.inset(),
					wrap = true,
					text_size = "$font.size.sm",
					line_height = "$font.line.sm",
					color = "$text.tertiary",
				}
				else nil,
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
											table.insert(cards, ui.div { w = w, shrink = 0, ds.Settings.FeatureSkeleton { still = still, cover = kind.cards } })
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
								return ui.each(featured_pages.items, function(first: client.Card, i: number, now: State<client.Card>): Node
									return ui.div { w = w, shrink = 0, feature(first, i, now) }
								end, {
									key = function(c: client.Card): string
										return c.id
									end,
								})
							end,
						},
				}
			end, { w_full = true })),
			-- The store's plugins: a section, its title how many what's asked
			-- matches (the store's `total`; "Plugins" while it isn't known, or
			-- from a store that doesn't count), its controls the category and the
			-- order; then the card: the search, the rows. (Its head is one
			-- height loading or loaded: nothing under it moves.)
			ui.column { id = "store-list", w_full = true, on_resize = function(size: { w: number, h: number })
				if math.abs(size.w - list_w:peek()) >= 0.5 then
					list_w:set(size.w)
				end
			end, ds.Settings.Section {
				title = function(): string
					local n = total:get()
					return if n and n > 0 then say("plugins.store.count", { n = n }):get() else say("plugins.store.list_title"):get()
				end,
				-- (What narrows and orders it, at the title's end; in a narrow
				-- pane, on a line of its own under the title.)
				trail = ui.show(head_on_one_line, head_controls(false)),
				ui.show(function(): boolean
					return not head_on_one_line()
				end, ui.row { w_full = true, px = ds.Settings.inset(), head_controls(true) }),
				table.unpack(list_body()),
			} },
		}
	end)

	-- For the page's link from elsewhere (a keyword on a plugin's page):
	-- Browse narrowed to it.
	function M.show_keyword(k: string)
		query:set("")
		category:set("")
		author:set("")
		keyword:set(k)
	end

	-- For a plugin's page's publisher: Browse narrowed to their plugins.
	function M.show_author(username: string)
		query:set("")
		category:set("")
		keyword:set("")
		author:set(username)
	end

	-- (Tests.)
	M._listing = listing

	return M
end

return C
