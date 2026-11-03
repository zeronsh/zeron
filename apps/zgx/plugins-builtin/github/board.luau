--!strict
-- Pull requests: a page of the GitHub plugin's own, in the sidebar
-- ("pages.add"), as Zeron's pull request board (crates/ui/src/
-- pull_requests.rs), drawn from Zeron's design system as it is lent to
-- plugins (`@ds`): a project's repository on GitHub, its open pull
-- requests under three filters (Authored, Reviewing, All), searched as you
-- type, newest updates first, a page at a time; a row opens the pull
-- request in the page's own panel (Zeron's right pane: resized, expanded,
-- tabbed as a chat's), the viewer drawing it (viewer.luau). GitHub is asked
-- through gh on this Mac (pulls.luau).
--
-- The page is a settings-like column: its title (and a way to ask again),
-- under it the project's picker and the repository it found, the filters
-- as the design system's tabs, and the pull requests as rows of one card
-- headed by its search (a quiet field, as the changes panel's filter):
-- each row its title, what it changes, its state and number, who opened
-- it (their GitHub picture by their name) and its status line (the board's
-- `row_status`), washed edge to edge under the pointer, the one open on
-- the selection wash. Nothing in the head needs room (a title and a chip),
-- so it holds at a pane's width; there the gutters tighten.
--
-- What it was looking at (the project, the filter) is kept between runs
-- (`zeron.store`), so a reload or a relaunch finds it as it was.

local git = require("@ds/git")
local Badge = require("@ds/badge")
local ListRow = require("@ds/list_row").ListRow
local Button = require("@ds/button")
local IconButton = require("@ds/icon_button").IconButton
local Text = require("@ds/text")
local Tabs = require("@ds/tabs")
local Dropdown = require("@ds/dropdown")
local SearchField = require("@ds/search_field")
local FadeText = require("@ds/fade_text")
local EmptyState = require("@ds/empty_state")
local banner = require("@ds/banner")
local skeleton = require("@ds/skeleton")
local people = require("@ds/people")
local control = require("@ds/control")

export type Item = {
	number: number,
	title: string,
	url: string,
	repository: string?,
	author: { login: string?, avatarUrl: string? }?,
	isDraft: boolean?,
	reviewDecision: string?,
	mergeability: string?,
	ci: { state: string? }?,
	additions: number?,
	deletions: number?,
}

local FILTER_KEYS = { "authored", "reviewing", "all" }

-- The plugin's words (the SDK's `s`), set as the board is made.
local T: any = nil

-- What the board says when a filter has nothing (Zeron's words).
local function empty_words(filter: string): { string }
	return { T.s(`board.empty.{filter}_title`), T.s(`board.empty.{filter}_body`) }
end

-- A kept first page younger than this is shown as is; an older one is
-- shown and asked again behind it (Zeron's five minutes: GitHub's rate
-- limits are tight).
local FRESH_SECONDS = 5 * 60

-- Under this width (a pane's), the page's gutters tighten.
local NARROW = 640

local CI: { [string]: string } = { failed = "ci_failing", pending = "ci_pending", passed = "ci_passed" }

-- A row's status line, as the board's (`row_status`): Draft, what blocks
-- it, Approved, and where its checks stand.
local function status(item: Item): string
	local out = {}
	if item.isDraft then
		table.insert(out, T.s("board.status.draft"))
	end
	if item.reviewDecision == "changesRequested" then
		table.insert(out, T.s("board.status.changes"))
	end
	if item.mergeability == "conflicting" then
		table.insert(out, T.s("board.status.conflicts"))
	end
	if item.reviewDecision == "approved" then
		table.insert(out, T.s("board.status.approved"))
	end
	local ci = CI[(item.ci and item.ci.state) or ""]
	if ci then
		table.insert(out, T.s(`board.status.{ci}`))
	end
	return table.concat(out, " · ")
end

-- Whether every word of `query` is in what the row says (the board's).
local function matches(item: Item, query: string): boolean
	local text = string.lower(`{item.title} {item.repository or ""} #{item.number} {status(item)} {(item.author and item.author.login) or ""}`)
	for word in string.gmatch(string.lower(query), "%S+") do
		if not string.find(text, word, 1, true) then
			return false
		end
	end
	return true
end

-- An ISO time's age in seconds (nil if it can't be read).
local function age(iso: string?): number?
	local y, mo, d, h, mi, se = string.match(iso or "", "^(%d+)-(%d+)-(%d+)T(%d+):(%d+):(%d+)")
	if not y then
		return nil
	end
	local t = os.time({ year = tonumber(y) :: number, month = tonumber(mo) :: number, day = tonumber(d) :: number, hour = tonumber(h) :: number, min = tonumber(mi) :: number, sec = tonumber(se) :: number })
	local utc_now = os.time(os.date("!*t") :: any)
	return utc_now - t
end

-- Someone's GitHub picture (as the viewer's): GitHub's address for it,
-- loaded as any remote image is ("network"), their gradient under it
-- until it's here.
local function who(login: string?, avatar: string?): people.Who
	local name = login or "someone"
	-- (GitHub's own address for it, on its picture servers; else its
	-- address by login, which GitHub sends on there.)
	local own = if type(avatar) == "string" and string.match(avatar, "^https://avatars%.githubusercontent%.com/") then avatar else nil
	local src = own or (if login and #login <= 39 and string.match(login, "^%w[%w%-]*$") then `https://github.com/{login}.png?size=96` else nil)
	return {
		id = name,
		name = name,
		src = if src
			then function(): string?
				return src
			end
			else nil,
	}
end

-- A pull request as a row of the board: its title and what it changes;
-- under them who opened it (their picture by their name) and where it
-- stands, its state and number at the end.
local function row_body(item: Item): Node
	local login = item.author and item.author.login
	local s = status(item)
	return ui.column {
		w_full = true,
		px = "$space.4",
		py = "$space.3",
		gap = "$space.1",
		ui.row {
			w_full = true,
			gap = "$space.2",
			items = "center",
			FadeText { item.title, grow = 1, min_w = 0, text_size = "$font.size.md", weight = "$font.weight.medium", color = "$text.primary" },
			Badge.DiffStat { added = item.additions or 0, removed = item.deletions or 0 },
		},
		ui.row {
			w_full = true,
			gap = "$space.2",
			items = "center",
			-- (Who first, at the row's edge, so every face stands in one
			-- column; the state and number at the end, under what it changes.)
			ui.row {
				grow = 1,
				min_w = 0,
				gap = "$space.1",
				items = "center",
				-- (No author known: no picture, the words where it would be.)
				ui.show(login ~= nil, people.face(who(login, item.author and item.author.avatarUrl), 16)),
				Text { if s ~= "" then `{login or ""} · {s}` else (login or ""), style = "caption", tone = "tertiary", truncate = true, grow = 1, min_w = 0 },
			},
			Badge.PullRequest { state = (if item.isDraft then "draft" else "open") :: any, number = item.number },
		},
	}
end

-- A row of the card: the whole row washes under the pointer, edge to edge
-- (the card's corner clips the first and the last), the open one on the
-- selection wash.
local Row = function(props: { item: Item, chosen: () -> boolean, on_click: () -> () }): Node
	local item = props.item
	-- (The design system's list row, ds/list_row: the one way a list's
	-- rows answer the pointer, here as in Zeron's own lists.)
	return ListRow {
		class = "github-board-row",
		label = `#{item.number} {item.title}`,
		selected = props.chosen,
		on_click = props.on_click,
		row_body(item),
	}
end

return function(zeron: any, pulls: any, _Viewer: any): (props: { page: any, panel: any? }) -> Node
	T = zeron
	local FILTERS = {}
	for _, f in FILTER_KEYS do
		table.insert(FILTERS, { value = f, label = zeron.s(`board.filters.{f}`) })
	end
	T = zeron
	return ui.component(function(props: { page: any, panel: any? }): Node
		local kept = zeron.store("board", { project = nil :: string?, filter = "authored" })
		local query = ui.use_state("")
		local repo = ui.use_state(nil :: string?)
		local items = ui.use_state({} :: { Item })
		local paging = ui.use_state({ next = nil :: string?, total = nil :: number? })
		local state = ui.use_state("idle")
		local failed = ui.use_state(nil :: string?)
		local open = ui.use_state(nil :: Item?)
		local width = ui.use_state(0)
		-- (Answers to an older ask are let go: the newest wins.)
		local asked = 0

		-- (What's chosen, as chosen now: storage answers a beat later.)
		local chosen = { project = (kept:peek() or {}).project, filter = (kept:peek() or {}).filter or "authored" }
		-- The picker's and the tabs' own states, following what's kept.
		local project_value = ui.use_state(chosen.project :: string?)
		local filter_value = ui.use_state(chosen.filter)

		-- The list, asked: kept (at once, if it's here), GitHub's anew
		-- (`refresh`), or the next page (`more`). `quiet`: asked behind what's
		-- showing, which stays (no skeleton), and stays if the ask fails.
		local function load(how: { refresh: boolean?, more: boolean?, quiet: boolean? }?)
			local h = how or {}
			local p, r = chosen.project, repo:peek()
			if not p or not r then
				return
			end
			asked += 1
			local mine = asked
			if not h.more and not h.quiet then
				state:set("loading")
			end
			pulls.list(r, {
				filter = chosen.filter,
				after = if h.more then paging:peek().next else nil,
				refresh = h.refresh == true,
				cached = not h.more and not h.refresh,
			}):after(function(page: any)
				if mine ~= asked then
					return
				end
				local list = if h.more then table.clone(items:peek()) else {}
				for _, item in page.items or {} do
					table.insert(list, item)
				end
				items:set(list)
				paging:set({ next = page.nextCursor, total = page.totalCount })
				failed:set(nil)
				state:set("ready")
				-- Kept, and older than five minutes: shown, and asked again.
				local old = age(page.fetchedAt)
				if not h.more and not h.refresh and old and old > FRESH_SECONDS then
					load({ refresh = true, quiet = true })
				end
			end, function(err: any)
				if mine ~= asked then
					return
				end
				if h.quiet and state:peek() == "ready" then
					return
				end
				failed:set(if type(err) == "table" and err.message then tostring(err.message) else tostring(err))
				state:set("failed")
			end)
		end

		-- The project's repository, then its list.
		local function find_repository()
			local p = chosen.project
			repo:set(nil)
			items:set({})
			open:set(nil)
			if not p then
				state:set("idle")
				return
			end
			state:set("loading")
			-- (Its folder on this Mac: gh says which repository it is.)
			local folder = (zeron.context_of("project", p) or {}).path
			pulls.repository(folder):after(function(r: any)
				if chosen.project ~= p then
					return
				end
				if type(r) == "string" and r ~= "" then
					repo:set(r)
					load()
				else
					state:set("norepo")
				end
			end, function(_why: any)
				if chosen.project ~= p then
					return
				end
				state:set("norepo")
			end)
		end
		find_repository()

		local function choose(p: string)
			if p == chosen.project then
				return
			end
			chosen.project = p
			kept:set({ project = p, filter = chosen.filter })
			find_repository()
		end
		local function set_filter(f: string)
			if f == chosen.filter then
				return
			end
			chosen.filter = f
			kept:set({ project = chosen.project, filter = f })
			load()
		end

		-- The projects to choose from (yours, by name).
		local function project_options(): { any }
			local sorted: { { value: string, label: string } } = {}
			for id, p in zeron.projects() do
				table.insert(sorted, { value = id, label = p.name or id })
			end
			table.sort(sorted, function(a, b)
				return string.lower(a.label) < string.lower(b.label)
			end)
			return sorted
		end

		-- The page's head: its title and a way to ask again; under it the
		-- project's picker (a chip) and the repository it shows (or what to
		-- do first).
		local function head(): Node
			return ui.column {
			w_full = true,
			gap = "$space.2",
			ui.row {
				w_full = true,
				gap = "$space.3",
				items = "center",
				Text { zeron.t("board.title"), style = "title", heading = 1, grow = 1, min_w = 0 },
				IconButton {
					icon = "refresh",
					label = zeron.s("board.refresh"),
					tone = "quiet",
					disabled = function(): boolean
						return repo:get() == nil
					end,
					on_click = function()
						load({ refresh = true, quiet = state:peek() == "ready" })
					end,
				},
			},
			ui.row {
				w_full = true,
				gap = "$space.2",
				items = "center",
				Dropdown {
					value = project_value,
					items = project_options,
					label = zeron.s("board.project"),
					icon = "folder",
					tone = "ghost",
					placeholder = zeron.s("board.choose_project"),
					search = zeron.s("board.search_projects"),
					on_choose = function(id: string)
						choose(id)
					end,
				},
				Text {
					function(): string
						return repo:get() or zeron.s("board.hint")
					end,
					style = "caption",
					tone = "tertiary",
					truncate = true,
					grow = 1,
					min_w = 0,
				},
			},
		}
		end

		-- The filters as the design system's tabs.
		local function filters(): Node
			local options = {}
			for _, f in FILTER_KEYS do
				table.insert(options, { value = f, label = zeron.s(`board.filters.{f}`) })
			end
			return ui.div {
			id = "github-board-filters",
			w_full = true,
			Tabs {
				value = filter_value,
				options = options,
				label = zeron.s("board.filter"),
				on_change = function(f: any)
					set_filter(f)
				end,
			},
		}
		end

		-- The search: a quiet field heading the card, its words on the rows'
		-- column; inset evenly, with the corner that inset gives it inside
		-- the card (for its focus ring), as the changes panel's filter.
		local function search(): Node
			return ui.div {
			w_full = true,
			shrink = 0,
			p = "$space.1",
			theme = { radius = { md = control.nest(git.RADIUS, git.EDGE, "$space.1") } },
			SearchField { value = query, placeholder = zeron.s("board.search"), name = zeron.s("board.search"), size = "sm", quiet = true, w_full = true },
		}
		end

		local function row(item: Item): Node
			return Row {
				item = item,
				chosen = function(): boolean
					local now = open:get()
					return now ~= nil and now.url == item.url
				end,
				on_click = function()
					-- In the page's panel, as a tab (one already open comes forward).
					open:set(item)
					if props.panel then
						props.panel.open_tab {
							kind = "github-pr",
							title = if item.title ~= "" then item.title else `#{item.number}`,
							icon = "pull-request",
							data = { url = item.url, number = item.number, project = chosen.project },
						}
					end
				end,
			}
		end

		-- The rows that match the search (or what it says when none do):
		-- drawn again as the search or the list changes, the search field
		-- above them staying as it is (it keeps the keys).
		local function rows(): Node
			return ui.presence(function(): string
				local q = query:get()
				local shown = 0
				for _, item in items:get() do
					if matches(item, q) then
						shown += 1
					end
				end
				return `{chosen.filter}:{q}:{shown}:{#items:get()}`
			end, function(_key: string): Node
				local q = query:peek()
				local list: { Node } = {}
				for _, item in items:peek() do
					if matches(item, q) then
						if #list > 0 then
							table.insert(list, git.rule())
						end
						table.insert(list, row(item))
					end
				end
				if #list == 0 then
					local said = if q ~= "" then { zeron.s("board.empty.nomatch_title", { q = q }), zeron.s("board.empty.nomatch_body") } else empty_words(chosen.filter)
					-- (In from the card's sides, as Settings.Catalog's empty states
					-- are: its words wrap inside the card, never against its edge.)
					return ui.column {
						w_full = true,
						py = "$space.6",
						px = "$space.4",
						EmptyState { icon = if q ~= "" then "search" else "pull-request", title = said[1], body = said[2] },
					}
				end
				return ui.column { w_full = true, table.unpack(list) }
			end, { w_full = true })
		end

		local function list(): Node
			return ui.presence(function(): string
				return state:get()
			end, function(key: string): Node
				if key == "idle" then
					return EmptyState {
						icon = "pull-request",
						title = zeron.s("board.empty.choose_title"),
						body = zeron.s("board.empty.choose_body"),
						fill = true,
					}
				elseif key == "loading" then
					local skeletons: { Node } = {}
					for _ = 1, 4 do
						table.insert(skeletons, skeleton.Row { lead = 24, title = 220, line = 140, trail = 48 })
					end
					return git.card(nil, skeletons)
				elseif key == "norepo" then
					return EmptyState {
						icon = "git-branch",
						title = zeron.s("board.empty.norepo_title"),
						body = zeron.s("board.empty.norepo_body"),
						fill = true,
					}
				elseif key == "failed" then
					return banner.Callout {
						tone = "danger",
						title = zeron.s("board.failed_title"),
						body = failed:peek() or "",
						action = {
							label = zeron.s("board.try_again"),
							on_click = function()
								load({ refresh = true })
							end,
						},
					}
				end
				return ui.column {
					id = "github-board-list",
					w_full = true,
					gap = "$space.3",
					git.card(search(), { rows() }),
					ui.row {
						w_full = true,
						gap = "$space.3",
						items = "center",
						Text {
							function(): string
								local total = paging:get().total
								return if total then zeron.s("board.count", { shown = #items:get(), total = total }) else ""
							end,
							style = "caption",
							tone = "tertiary",
							grow = 1,
						},
						ui.show(
							function(): boolean
								return paging:get().next ~= nil
							end,
							Button {
								zeron.t("board.load_more"),
								tone = "ghost",
								size = "sm",
								on_click = function()
									load({ more = true })
								end,
							}
						),
					},
				}
			end, { w_full = true, grow = 1, enter = { opacity = { 0, 1 }, duration = 120 }, exit = { opacity = 0, duration = 60 }, mode = "wait" })
		end

		local function gutter(): string
			local w = width:get()
			return if w > 0 and w < NARROW then "$space.4" else "$space.6"
		end

		local function board_page(): Node
			return ui.column {
				class = "github-board",
				w_full = true,
				h_full = true,
				on_resize = function(size)

					-- (No width at all is no width: the page's column closing.)
					local w = math.floor(size.w)
					if w > 0 and w ~= width:peek() then
						width:set(w)
					end
				end,
				ui.scroll {
					w_full = true,
					h_full = true,
					fade = { 16, 16 },
					fade_overflow = true,
					ui.column {
						w_full = true,
						min_h = "100%",
						items = "center",
						px = gutter,
						pt = gutter,
						pb = "$space.6",
						-- (A column the gallery's git pages' width at most, centred: on
						-- a wide window the rows hold together rather than stretch.)
						ui.column {
							w_full = true,
							max_w = ui.raw(960, "the gallery's git window"),
							gap = "$space.5",
							head(),
							filters(),
							list(),
						},
					},
				},
			}
		end

		-- (The language shown: tabs and placeholders take words, not
		-- functions, so the page is built again as it changes.)
		local locale = ui.shared("zeron.locale", "en")
		return ui.presence(function(): string
			local l = locale:get()
			return if type(l) == "string" then l else "en"
		end, function(): Node
			return board_page()
		end, { w_full = true, h_full = true })
	end)
end
