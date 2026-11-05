-- Settings → Browser → History: the pages you've visited in the panel's
-- browser, kept for your account on this Mac (screens/panel/browser_data).
-- One card for what's kept (whether, and clearing a span of it); under it
-- the pages themselves, searched from one field and laid out by day, as a
-- browser's history is read: each row the site's icon (its initial until
-- the tab has fetched one), the page's title and address, and the time it
-- was visited, at the row's end in the clock's figures. A page's × shows as
-- the row is pointed at or reached from the keys, so a long list reads as
-- pages, not as a column of buttons.

local ds = require("@ds")
local toast = require("@ds/toast")
local locale = require("@ds/locale")
local IconButton = require("@ds/icon_button").IconButton
local search = require("@app/screens/settings/search")
local data = require("@app/screens/panel/browser_data")

local M = {}

M.ID = "browser_history"
M.TITLE = "History"

search.index(M.ID, ui.t("settings.browser_history.title"), ui.t("settings.browser.history_terms"), {
	{ key = "keep_history", title = ui.t("settings.browser.keep_history.label"), terms = ui.t("settings.browser.keep_history.terms") },
	{ key = "clear_history", title = ui.t("settings.browser.clear_history.label"), terms = ui.t("settings.browser.clear_history.terms") },
	{ key = "history", title = ui.t("settings.browser.history_search"), terms = ui.t("settings.browser.history_terms") },
})

local function row(key: string, props: any): Node
	return search.anchor(key, ds.Settings.Row(props))
end

-- The spans history clears over, and how far back each reaches.
local SPANS: { [string]: number? } = { hour = 3600, day = 86400, week = 7 * 86400, month = 28 * 86400, all = nil }
local function span_options(): { { value: string, label: string } }
	return {
		{ value = "hour", label = ui.t("settings.browser.span.hour"):get() },
		{ value = "day", label = ui.t("settings.browser.span.day"):get() },
		{ value = "week", label = ui.t("settings.browser.span.week"):get() },
		{ value = "month", label = ui.t("settings.browser.span.month"):get() },
		{ value = "all", label = ui.t("settings.browser.span.all"):get() },
	}
end
local span = ui.state("settings_browser_span", "hour")
local query = ui.state("settings_browser_history_query", "")
-- How many visits the list shows (more as asked).
local SHOWN = 50
local shown = ui.state("settings_browser_history_shown", SHOWN)

-- The visits matching the filter, newest first, as many as shown.
local visits = ui.computed(function(): { data.Visit }
	local profile = data.profile()
	if not profile then
		return {}
	end
	local q = string.lower(string.match(query:get(), "^%s*(.-)%s*$") or "")
	local most = shown:get()
	local out: { data.Visit } = {}
	for _, v in data.history(profile) do
		if q == "" or string.find(string.lower(`{v.url} {v.title or ""}`), q, 1, true) then
			table.insert(out, v)
			if #out >= most then
				break
			end
		end
	end
	return out
end)
local function total(): number
	local profile = data.profile()
	return if profile then #data.history(profile) else 0
end

-- The start of `at`'s day, on this Mac's clock.
local function day_of(at: number): number
	local d = os.date("*t", math.floor(at)) :: any
	d.hour, d.min, d.sec = 0, 0, 0
	return os.time(d)
end

-- What a day is called over its pages: Today, Yesterday, else its date
-- (with its year only when it isn't this one's).
local function day_name(start: number, today: number): string
	if start == today then
		return ui.t("settings.browser_history.today"):get()
	elseif start == day_of(today - 43200) then
		return ui.t("settings.browser_history.yesterday"):get()
	end
	local this_year = (os.date("*t", today) :: any).year
	local year = (os.date("*t", start) :: any).year
	return if year == this_year
		then `{locale.weekday(start)}, {locale.month_day(start)}`
		else locale.date(start)
end

type Day = { key: string, name: string, visits: { data.Visit } }

-- The visits shown, by day, the latest day first.
local days = ui.computed(function(): { Day }
	local today = day_of(os.time())
	local out: { Day } = {}
	local by: { [number]: Day } = {}
	for _, v in visits:get() do
		local start = day_of(v.at)
		local day = by[start]
		if not day then
			day = { key = tostring(start), name = day_name(start, today), visits = {} }
			by[start] = day
			table.insert(out, day)
		end
		table.insert(day.visits, v)
	end
	return out
end)

function M.clear(which: string)
	local profile = data.profile()
	if not profile then
		return
	end
	local back = SPANS[which]
	data.clear_history(profile, if back then os.time() - back else nil)
	toast.show { title = ui.t("settings.browser.cleared"):get(), tone = "success" }
end

local function host_of(url: string): string
	return string.match(url, "^%a[%w+.-]*://([^/?#:]+)") or url
end

-- A page's row (its visit followed in place: a title arriving, its icon).
local VisitRow = ui.component(function(props: { visit: data.Visit, now: State<any> }): Node
	local now = props.now
	local over = ui.use_state(false)
	local focused = ui.use_state(false)
	local function it(): data.Visit
		return now:get() :: data.Visit
	end
	local function lit(): boolean
		return over:get() or focused:get()
	end
	return ui.row {
		class = "browser-visit",
		w_full = true,
		on_hover = function(inside: boolean)
			over:set(inside)
		end,
		ds.Settings.Row {
			truncate = true,
			lead = ds.Tile {
				size = 20,
				label = function(): string?
					return data.site_name(host_of(it().url))
				end,
				src = function(): string?
					return it().icon
				end,
			},
			label = function(): string
				local v = it()
				return if v.title and v.title ~= "" then v.title else data.short(v.url)
			end,
			description = function(): string
				return data.short(it().url)
			end,
			-- When it was visited, on the controls' edge; pointed at (or
			-- reached from the keys), the × to let it go in its place.
			control = ui.div {
				relative = true,
				items = "end",
				justify = "center",
				h = "$zeron.icon_button.sm",
				ui.text {
					function(): string
						return locale.clock(it().at)
					end,
					literal = true,
					tabular = true,
					text_size = "$font.size.sm",
					color = "$text.tertiary",
					opacity = function(): number
						return if lit() then 0 else 1
					end,
					aria_hidden = true,
				},
				ui.div {
					absolute = true,
					right = 0,
					top = 0,
					IconButton {
						icon = "close",
						size = "sm",
						label = ui.t("settings.browser.forget", { page = props.visit.title or data.short(props.visit.url) }),
						opacity = function(): number
							return if lit() then 1 else 0
						end,
						on_focus = function()
							focused:set(true)
						end,
						on_blur = function()
							focused:set(false)
						end,
						on_click = function()
							local profile = data.profile()
							if profile then
								data.forget(profile, props.visit.url)
							end
						end,
					},
				},
			},
		},
	}
end)

local function visit_row(v: data.Visit, _i: number, now: State<any>): Node
	return VisitRow { visit = v, now = now }
end

-- A day: its name over its card of pages.
local function day_block(day: Day, _i: number, now: State<any>): Node
	local pages = ui.computed(function(): { data.Visit }
		return (now:get() :: Day).visits
	end)
	return ui.column {
		class = "browser-history-day",
		w_full = true,
		gap = "$space.2",
		ui.row {
			w_full = true,
			px = ds.Settings.inset(),
			ds.SectionTitle {
				function(): string
					return (now:get() :: Day).name
				end,
			},
		},
		-- (Each washed edge to edge under the pointer, as every list of
		-- things in a card is: Settings.CatalogRow.)
		ds.Settings.Each(pages, visit_row, {
			key = function(v: data.Visit): string
				return v.url
			end,
			wash = true,
		}),
	}
end

function M.view(): Node
	return ds.Settings.Page {
		title = ui.t("settings.browser_history.title"),
		description = ui.t("settings.browser.history_description"),
		ds.Settings.Section {
			ds.Settings.Group {
				row("keep_history", {
					label = ui.t("settings.browser.keep_history.label"),
					description = ui.t("settings.browser.keep_history.description"),
					control = ds.Switch { value = data.keep_history, label = ui.t("settings.browser.keep_history.label") },
				}),
				row("clear_history", {
					label = ui.t("settings.browser.clear_history.label"),
					description = function(): string
						return ui.t("settings.browser.clear_history.description", { n = total() }):get()
					end,
					control = ui.row {
						gap = "$space.2",
						items = "center",
						ui.div {
							id = "settings-browser-span",
							ds.Select { value = span, name = ui.t("settings.browser.clear_history.span"), w = 144, options = span_options },
						},
						ds.Button {
							id = "settings-browser-clear",
							tone = "secondary",
							disabled = function(): boolean
								return total() == 0
							end,
							on_click = function()
								M.clear(span:peek())
							end,
							ui.text { ui.t("settings.browser.clear_history.button") },
						},
					},
				}),
			},
		},
		search.anchor("history", ui.column {
			w_full = true,
			gap = "$space.5",
			-- Nothing kept: what will be here, or that it's off.
			ui.show(function(): boolean
				return total() == 0
			end, ui.column {
				w_full = true,
				py = "$space.6",
				ds.EmptyState {
					icon = "clock-circle",
					size = "sm",
					title = ui.t("settings.browser_history.empty_title"),
					body = function(): string
						return if data.keep_history:get()
							then ui.t("settings.browser_history.empty"):get()
							else ui.t("settings.browser_history.off"):get()
					end,
				},
			}),
			ui.show(function(): boolean
				return total() > 0
			end, ds.SearchField {
				value = query,
				name = ui.t("settings.browser.history_search"),
				placeholder = ui.t("settings.browser.history_search"),
				w_full = true,
				results = function(): number
					return #visits:get()
				end,
			}),
			ui.each(days, day_block, {
				key = function(day: Day): string
					return day.key
				end,
				direction = "column",
				gap = "$space.5",
				w_full = true,
			}),
			ui.show(function(): boolean
				return total() > 0 and #visits:get() == 0
			end, ui.row {
				w_full = true,
				justify = "center",
				py = "$space.4",
				ds.Hint { ui.t("settings.browser.history_none_match") },
			}),
			ui.show(function(): boolean
				return #visits:get() >= shown:get()
			end, ui.row {
				w_full = true,
				justify = "center",
				ds.Button {
					id = "settings-browser-more",
					tone = "ghost",
					size = "sm",
					on_click = function()
						shown:set(shown:peek() + SHOWN)
					end,
					ui.text { ui.t("settings.browser.history_more") },
				},
			}),
		}),
	}
end

return M
