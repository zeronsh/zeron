-- Settings, in the main window's page: its pages listed at the left, the
-- one open beside them (the route's `settings/:page`). The shell's own is
-- Archived; screens/settings adds the rest (General, Account, …), and
-- others add theirs:
--
--   local settings = require("@app/screens/shell/settings")
--   settings.add { id = "plugins", title = "Plugins", icon = "widget", view = function() return … end }
--
-- An app module adds a page as it loads (the Plugins page does), in the
-- order added, after the app's own; `before = "account"` puts it ahead of
-- one; `group = "Personal"` lists it under that heading. `settings.head(fn)`
-- puts what `fn` builds over the rows (the settings search's field).
-- Plugins add rows under the list in its slot (`settings.nav`) and place
-- by `#settings-nav`, `.settings-nav-row` and `#settings-page`. Account is
-- out of their reach.
--
-- Pages that come and go (a plugin's settings, while it's on) are listed
-- by `settings.extra(name, fn)`: `fn` returns them as they are now (read
-- where it reads states, so the list follows), after the app's own.
--
-- `settings.View()`: settings, a page of its own over the whole window (the
-- shell shows it while the route is `settings/:page`).
--
-- `settings.title_of(id, fallback)`: a page's title in the locale shown, a
-- string read now (`title` stays the English it's kept and compared by).

local ds = require("@ds")
local route = require("@app/screens/shell/route")

local M = {}

export type Page = {
	id: string,
	title: string,
	icon: string?,
	-- What it shows: built as it opens (a page another opens in its place).
	view: () -> Node,
	-- Put ahead of this page (an id), else after the rest.
	before: string?,
	-- Out of plugins' reach (an account's, anything that spends or
	-- deletes).
	private: boolean?,
	-- The heading it's listed under ("Personal", "Devices", …): a heading
	-- shows where the group changes, down the list's order.
	group: string?,
	-- In the icon's place in the list: a tile (a plugin's, as its row on
	-- the Plugins page has it).
	tile: { label: string, hue: string? }?,
}

-- The pages: their ids in order (a state, so the list follows them), and
-- each by id (a view is a function: state holds data alone).
local order = ui.state("shell_settings_pages", {} :: { string })
local pages: { [string]: Page } = {}
M.pages = order

-- Pages that come and go, by who lists them (`M.extra`).
local extras: { [string]: () -> { Page } } = {}
local extra_names: { string } = {}

function M.extra(name: string, list: () -> { Page })
	if not extras[name] then
		table.insert(extra_names, name)
	end
	extras[name] = list
end

-- Every page that comes and goes, now (followed where it's read).
local function extra_pages(): { Page }
	local out: { Page } = {}
	for _, name in extra_names do
		for _, page in extras[name]() do
			table.insert(out, page)
		end
	end
	return out
end

-- A page added, by its id (or one listed by `extra`, while it's there).
function M.page(id: string): Page?
	if pages[id] then
		return pages[id]
	end
	for _, page in extra_pages() do
		if page.id == id then
			return page
		end
	end
	return nil
end

-- What heads the list, under "Back to chats" and over the pages (the
-- settings search's field), built with the list; while `opts.replaces()`,
-- the pages are put away (its results, drawn in it, stand in their place).
local head: (() -> Node)? = nil
local replaces: (() -> boolean)? = nil
function M.head(build: () -> Node, opts: { replaces: (() -> boolean)? }?)
	head = build
	replaces = if opts then opts.replaces else nil
end

-- Where the open page is scrolled (a search's hit sets it, to land on its
-- row); a page chosen in the list opens at its top.
M.scroll = ui.state("shell_settings_scroll", 0)

function M.add(page: Page)
	pages[page.id] = page
	-- (Listed once: the list outlives a reload of the scripts, as the
	-- pages are added again.)
	if table.find(order:peek(), page.id) then
		return
	end
	local ids = table.clone(order:peek())
	local at = if page.before then table.find(ids, page.before) else nil
	table.insert(ids, at or #ids + 1, page.id)
	order:set(ids)
end

-- The list's width (the sidebar's least), and the title bar's strip over
-- the window (Zeron's: the window's buttons sit in it).
local TITLE_H = 38
-- The group the pages that come and go list ahead of.
local EXTRAS_BEFORE = "Zeron"

-- Archived: sessions put away (screens/settings/archived), last.
M.add {
	id = "archived",
	title = "Archived",
	icon = "archive-minimalistic",
	-- (Under a heading of its own, last: Zeron keeps it in its last run of
	-- pages, apart from the account's and the organization's.)
	group = "Workspace",
	view = function(): Node
		return require("@app/screens/settings/archived").view()
	end,
}

-- What "Back to chats" is among the list's values.
local BACK = "::back"

-- The list's names in the language shown, looked up as it's drawn: a
-- page's title and a group's stay English where they're kept and compared.
-- A page by its id, and only while it's called what the app calls it (a
-- page another adds, or one in a page's place under another name, keeps
-- the name it was given); a heading by its English.
local TITLE_KEYS: { [string]: { string } } = {
	general = { "General", "settings.general.title" },
	appearance = { "Appearance", "settings.appearance.title" },
	notifications = { "Notifications", "settings.notifications.title" },
	files = { "Files", "settings.files.title" },
	browser = { "Browsing", "settings.browser.page_title" },
	browser_history = { "History", "settings.browser_history.title" },
	shortcuts = { "Shortcuts", "settings.shortcuts.title" },
	appshots = { "Appshots", "settings.appshots.title" },
	providers = { "Providers", "settings.providers.title" },
	devices = { "Devices", "settings.devices.title" },
	members = { "Members", "settings.members.title" },
	profile = { "Account", "settings.profile.title" },
	cloud = { "Cloud", "settings.cloud.title" },
	billing = { "Billing", "settings.billing.title" },
	about = { "About & updates", "settings.about.title" },
	archived = { "Archived", "settings.archived.title" },
	plugins = { "Plugins", "plugins.page.title" },
	themes = { "Themes", "themes.page.title" },
	templates = { "Templates", "templates.page.title" },
}
local GROUP_KEYS: { [string]: string } = {
	Personal = "settings.shell.group.personal",
	Browser = "settings.shell.group.browser",
	Devices = "settings.shell.group.devices",
	Organization = "settings.shell.group.organization",
	Workspace = "settings.shell.group.workspace",
	Cloud = "settings.shell.group.cloud",
	Customize = "settings.shell.group.customize",
	Plugins = "settings.shell.group.plugins",
}
local function title_of(id: string, title: string): string
	local known = TITLE_KEYS[id]
	return if known and known[1] == title then ui.t(known[2]):get() else title
end
local function group_of(group: string): string
	local key = GROUP_KEYS[group]
	return if key then ui.t(key):get() else group
end

-- A page's title in the locale shown, read now (tracked in a function or
-- computed): the page added by that id, else `fallback` (its English), as
-- the list draws it.
function M.title_of(id: string, fallback: string?): string
	local page = M.page(id)
	return title_of(id, if page then page.title else (fallback or id))
end

-- Settings, a page of its own over the whole window: its pages listed
-- down the window's full height (the window's buttons in the list's top
-- strip, as a macOS sidebar has them), "Back to chats" at the list's head,
-- the page scrolling beside it under a strip of its own. The gallery's
-- settings window (demos/example_kit `settings`). Esc, ⌘[ and Back to
-- chats go back where the chats were, as they were.
function M.View(): Node
	-- Where the page is scrolled, as the view has it: `M.scroll` as it was,
	-- put back until a layout keeps it. (A view made again, the scripts
	-- reloaded, lays the page out before its rows have come; its scroll,
	-- clamped to that, would otherwise write 0 over where it was.) Once
	-- it holds, each follows the other: a scroll here, a search's reveal
	-- there.
	-- Put back as the page lays out (its rows coming), until a layout has
	-- room for it; or, never that tall now, as soon as the person scrolls.
	local target = M.scroll:peek()
	local shown = ui.use_state(target)
	local settled = target <= 0
	local view = ui.ref()
	local function laid_out(h: number)
		if settled then
			return
		end
		shown:set(target)
		if h - (view.height or 0) >= target - 0.5 then
			settled = true
			M.scroll:set(target)
		end
	end
	local function scrolled(): boolean
		if not settled then
			settled = true
			M.scroll:set(shown:peek())
		end
		return false
	end
	ui.effect(function()
		local y = shown:get()
		if settled and math.abs(M.scroll:peek() - y) > 0.5 then
			M.scroll:set(y)
		end
	end)
	ui.effect(function()
		local want = M.scroll:get()
		if settled and math.abs(want - shown:peek()) > 0.5 then
			shown:set(want)
		end
	end)
	return ui.row {
		id = "settings",
		w_full = true,
		h_full = true,
		min_h = 0,
		-- The pages (the design system's list of them; a page added later
		-- joins it in place), and what plugins add under them.
		ui.div {
			id = "settings-nav",
			hook = "settings.nav",
			h_full = true,
			shrink = 0,
			relative = true,
			ds.Settings.Nav {
				value = ui.computed(function(): string?
					return route.current:get().page
				end) :: any,
				on_choose = function(id: string)
					if id == BACK then
						route.leave_settings()
					else
						M.scroll:set(0)
						route.settings(id)
					end
				end,
				-- (The main sidebar's width, shared and dragged in either; its
				-- bounds. Asked for here: both are loaded by now.)
				width = require("@app/screens/shell/sidebar").width,
				min_w = require("@app/screens/shell/fit").SIDEBAR_MIN,
				max_w = require("@app/screens/shell/fit").SIDEBAR_MAX,
				top = TITLE_H,
				lead = { { value = BACK, label = ui.t("settings.shell.back") :: any, icon = "arrow-left" } },
				items = function(): { any }
					local out: { any } = {}
					-- (Under one heading, "Settings", while no page has a group
					-- of its own; else each group's, where the group changes.)
					local grouped = false
					for _, id in order:get() do
						grouped = grouped or (pages[id] ~= nil and pages[id].group ~= nil)
					end
					if not grouped then
						table.insert(out, { heading = ui.t("settings.shell.heading"):get() })
					end
					local last: string? = nil
					-- Those that come and go (plugins' pages), under their own
					-- headings: ahead of the "Zeron" group (About and updates
					-- stay last), or at the end where there's none.
					local extras_in = false
					local function add_extras()
						if extras_in then
							return
						end
						extras_in = true
						for _, page in extra_pages() do
							if page.group and page.group ~= last then
								table.insert(out, { heading = group_of(page.group) })
							end
							last = page.group
							table.insert(out, { value = page.id, label = title_of(page.id, page.title), icon = page.icon, tile = page.tile, class = "settings-nav-row" })
						end
					end
					for _, id in order:get() do
						local page = pages[id]
						if page then
							if page.group == EXTRAS_BEFORE then
								add_extras()
							end
							if page.group and page.group ~= last then
								table.insert(out, { heading = group_of(page.group) })
							end
							last = page.group
							table.insert(out, { value = id, label = title_of(id, page.title), icon = page.icon, class = "settings-nav-row" })
						end
					end
					add_extras()
					return out
				end,
				head = if head then head() else nil,
				hide_rows = replaces,
				foot = ui.slot("settings.nav", { w_full = true, gap = "$zeron.list.gap" }),
				-- You and settings, as the main sidebar's foot: you open your
				-- profile; settings' button, on here, closes them.
				account = (function(): Node?
					local sidebar: any = require("@app/screens/shell/sidebar")
					if not sidebar.account then
						return nil
					end
					return ds.AccountRow {
						name = function(): string
							return sidebar.user().name
						end,
						id = function(): string?
							return sidebar.user().id
						end,
						avatar = function(): string?
							return (sidebar.user() :: any).avatar
						end,
						on_user = sidebar.account.on_user,
						user_open = sidebar.account.user_open,
						on_settings = sidebar.account.on_settings,
						settings_open = sidebar.account.settings_open,
					}
				end)(),
			},
			-- Its strip, with the window's buttons in it, moves the window.
			ui.div { id = "settings-nav-drag", absolute = true, left = 0, right = 0, top = 0, h = TITLE_H, window_drag = true },
		},
		-- The page open, under its own strip (another page is another page:
		-- it swaps).
		ui.column {
			grow = 1,
			min_w = 0,
			h_full = true,
			-- (Its strip moves the window, a double click zooming it.)
			ui.div { id = "settings-drag", h = TITLE_H, w_full = true, shrink = 0, window_drag = true },
			ui.scroll {
				w_full = true,
				grow = 1,
				min_h = 0,
				fade = { 16, 16 },
				fade_overflow = true,
				offset = shown,
				ref = view,
				on_wheel = scrolled,
				ui.column {
					id = "settings-page",
					hook = "settings.page",
					-- (Laid out, as tall as it is: where it was scrolled again, the
					-- scroll's room known now.)
					on_resize = function(size)
						laid_out(size.h)
					end,
					w_full = true,
					px = "$space.6",
					pt = "$space.4",
					pb = "$space.8",
					items = "center",
					ui.presence(function(): string
						local id = route.current:get().page
						if id and M.page(id) then
							return id
						end
						-- (A page gone, or not added yet: the first one.)
						for _, first in order:get() do
							if pages[first] then
								return first
							end
						end
						return ""
					end, function(id: string): Node
						if id == "" then
							return ui.div {}
						end
						local page = M.page(id) :: Page
						-- Every page centred in the room beside the list, however
						-- it's built (the presence's own `items` doesn't reach
						-- inside a page's node); a private one out of plugins' reach.
						return ui.column {
							hook = if page.private then false else nil,
							w_full = true,
							items = "center",
							page.view(),
						}
					end, { w_full = true, items = "center" }),
				},
			},
		},
	}
end

return M
