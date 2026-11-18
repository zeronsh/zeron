-- Settings: plugins. What's installed, what's new, and what could be: a
-- real page's search, tabs, updates, and its loading, empty and failed states.
-- Browse is the store's catalog on the page's own pieces: the featured
-- (`Settings.Feature` on a `Shelf`), then the list's section, how many as
-- its title and the order its action, its card with a keyword narrowed to
-- beside its search and the categories (`Pager` paging them when they don't
-- fit) (`Settings.CatalogCard`), its rows built as they show
-- (`Settings.ListRows`);
-- a plugin opens a page under it (`Settings.SubPage`, `Settings.Back`).

local ds = require("@ds")
local Markdown = require("@ds/markdown")
local kit = require("@demos/example_kit")

local tab = ui.state("ex_pl_tab", "installed")
local query = ui.state("ex_pl_query", "")
local shown = ui.state("ex_pl_state", "ready")
local removing = ui.state("ex_pl_removing", false)
local category = ui.state("ex_pl_category", "")
local order = ui.state("ex_pl_order", "popular")
local keyword = ui.state("ex_pl_keyword", "design")
local opened = ui.state("ex_pl_opened", nil :: string?)
local enabled = {
	linear = ui.state("ex_pl_linear", true),
	sentry = ui.state("ex_pl_sentry", true),
	postgres = ui.state("ex_pl_postgres", false),
	playwright = ui.state("ex_pl_playwright", true),
}

local INSTALLED = {
	{ id = "linear", name = "Linear", by = "Zeron", hue = "indigo", version = "2.4.1", about = "Read issues and move them along from a chat." },
	{ id = "sentry", name = "Sentry", by = "Functional Software", hue = "rose", version = "1.9.0", update = "1.10.0", about = "Pull a stack trace and its release into the conversation." },
	{ id = "postgres", name = "Postgres", by = "Zeron", hue = "sky", version = "0.8.3", about = "Let agents read a schema and run read-only queries." },
	{ id = "playwright", name = "Playwright", by = "Microsoft", hue = "emerald", version = "3.2.0", about = "Drive a browser to check what an agent built." },
}
local CATALOG = {
	{ id = "figma", name = "Figma", by = "figma", hue = "orange", version = "1.2.0", installs = 4210, about = "Read frames and variables from a design file." },
	{ id = "slack", name = "Slack", by = "salesforce", hue = "purple", version = "4.0.2", installs = 3874, about = "Hear when an agent needs you, where you already are." },
	{ id = "datadog", name = "Datadog", by = "datadog", hue = "violet", version = "0.5.1", installs = 1290, about = "Bring a monitor's graph and its logs into a chat." },
}

local function matches(p: any): boolean
	local q = string.lower(query:get())
	return q == "" or string.find(string.lower(p.name .. " " .. p.about), q, 1, true) ~= nil
end

-- The plugins the tab holds that the search finds.
local found = ui.computed(function(): { any }
	local out = {}
	for _, p in (if tab:get() == "installed" then INSTALLED else CATALOG) do
		if matches(p) then
			table.insert(out, p)
		end
	end
	return out
end)

-- Browse: the featured over the catalog's card, its search, a bar of
-- filters, and its rows; a row opens the plugin's page.
local function browse(): Node
	local function feature(p: any): Node
		return ds.Settings.Feature {
			label = p.name,
			byline = `by @{p.by}`,
			description = p.about,
			lead = ds.Tile { label = p.name, hue = p.hue, size = 32 },
			on_click = function()
				opened:set(p.id)
			end,
		}
	end
	local ORDERS = { popular = "Most installed", updated = "Recently updated", new = "Newest" }
	return ui.column {
		w_full = true,
		gap = "$space.6",
		-- The featured on a shelf (ds.Shelf): three across while each keeps
		-- its least, else running sideways, the next cut by the column's end.
		ui.show(function(): boolean
			return query:get() == ""
		end, ds.Settings.Section {
			title = "Featured",
			ds.Shelf {
				across = 3,
				min_w = ds.Settings.FEATURE_MIN,
				count = function(): number
					return 3
				end,
				build = function(w: () -> number): any
					local cards = {}
					for i = 1, 3 do
						table.insert(cards, ui.div { w = w, shrink = 0, feature(CATALOG[i]) })
					end
					return cards
				end,
			},
		}),
		-- The list: a section, how many as its title, the order its quiet
		-- action; the card's search, its categories (ds.Pager pages them
		-- when they don't fit), its rows.
		ds.Settings.Section {
			title = function(): string
				return `{#found:get()} plugins`
			end,
			trail_on_line = true,
			trail = ds.Dropdown {
				value = order :: any,
				search = false,
				plain = true,
				placement = { side = "bottom", align = "end" },
				items = {
					{ value = "popular", label = "Most installed" },
					{ value = "updated", label = "Recently updated" },
					{ value = "new", label = "Newest" },
				},
				trigger = function(menu: any): Node
					return ds.Button {
						tone = "ghost",
						size = "xs",
						on_click = menu.toggle,
						ui.row {
							gap = "$space.1",
							items = "center",
							ui.text {
								function(): string
									return ORDERS[order:get()] or ""
								end,
							},
							ds.Icon { name = "alt-arrow-down", size = 14, aria_hidden = true },
						},
					}
				end,
			},
			ds.Settings.CatalogCard {
				search = { value = query, placeholder = "Search plugins" },
				search_trail = ui.show(function(): boolean
					return keyword:get() ~= ""
				end, ds.Tag {
					function(): string
						return keyword:get()
					end :: any,
					size = "sm",
					selected = true,
					on_remove = function()
						keyword:set("")
					end,
				}),
				-- (An xs tab's words are 8 inside it, so 8 here puts the
				-- first's on the rows' 16.)
				filters = ui.row {
					w_full = true,
					py = "$space.1",
					px = "$space.2",
					ds.Pager {
						back = "Earlier categories",
						more = "More categories",
						ds.Tabs {
							value = category,
							label = "Category",
							look = "pill",
							size = "xs",
							options = {
								{ value = "", label = "All" },
								{ value = "agents", label = "Agents" },
								{ value = "appearance", label = "Appearance" },
								{ value = "chat", label = "Chat" },
								{ value = "developer-tools", label = "Developer tools" },
								{ value = "git", label = "Git" },
								{ value = "integrations", label = "Integrations" },
								{ value = "productivity", label = "Productivity" },
								{ value = "utilities", label = "Utilities" },
							},
						},
					},
				},
				body = ds.Settings.ListRows(found, function(p: any): Node
					return ds.Settings.Row {
						label = p.name,
						byline = `by @{p.by} · {p.installs} installs`,
						description = p.about,
						lead = ds.Tile { label = p.name, hue = p.hue, size = 32 },
						truncate = true,
						value = if p.id == "slack" then "Installed" else "",
						on_click = function()
							opened:set(p.id)
						end,
					}
				end, {
					key = function(p: any): string
						return p.id
					end,
					height = function(): number
						return ds.Settings.truncated_row_h(32, true)
					end,
				}),
			},
		},
	}
end

-- A plugin of the store, opened: a page under Plugins.
local function listing(id: string): Node
	local p = CATALOG[1]
	for _, c in CATALOG do
		if c.id == id then
			p = c
		end
	end
	return ds.Settings.SubPage {
		where = ds.Settings.Back {
			label = "Plugins",
			on_click = function()
				opened:set(nil)
			end,
		},
		ds.Settings.Page {
			title = p.name,
			description = p.about,
			trail = ds.Tile { label = p.name, hue = p.hue, size = 48 },
			ds.Settings.Group {
				ui.column {
					w_full = true,
					px = "$space.4",
					py = "$space.3",
					ds.KeyValue { items = { { key = "By", value = `@{p.by}` }, { key = "Version", value = p.version }, { key = "Installs", value = `{p.installs} installs` } }, key_width = 96 },
				},
			},
			ui.row { w_full = true, justify = "end", ds.Button { "Install", tone = "primary" } },
			-- Its README as a document under the page's title (ds/markdown's
			-- `heading_step`, `drop_title`, `picture_radius`): its `# Name`
			-- not drawn again, its headings a level down in size, none larger
			-- than the page's name, each with its own level's room above (its
			-- `## Settings` 12 under the words before it, as in a chat reply).
			ds.Settings.Section {
				title = "About",
				ds.Settings.Group {
					ui.column {
						w_full = true,
						p = "$space.4",
						Markdown {
							text = `# {p.name}\n\n{p.about}\n\n# Getting started\n\nSign in once, then ask for it by name in any chat.\n\n## Settings\n\nNone: it reads what you already allowed.`,
							foreign = true,
							heading_step = 1,
							drop_title = function(words: string): boolean
								return string.lower(words) == string.lower(p.name)
							end,
							picture_radius = "$radius.lg",
						},
					},
				},
			},
		},
	}
end

local function row(p: any, installed: boolean): Node
	local control: Node
	if installed then
		control = ui.row {
			gap = "$space.2",
			items = "center",
			if p.update then ds.Button { "Update", tone = "secondary", size = "sm" } else nil,
			ds.Switch { value = enabled[p.id], label = p.name },
			ds.IconButton { icon = "trash-bin-minimalistic", label = `Remove {p.name}`, size = "sm", tone = "quiet", on_click = function() removing:set(true) end },
		}
	else
		control = ds.Button { "Install", tone = "secondary", size = "sm" }
	end
	return ds.Settings.Row {
		label = p.name,
		description = p.about,
		lead = ds.Tile { label = p.name, hue = p.hue, size = 32 },
		badge = if p.update
			then ds.Badge { `{p.version} → {p.update}`, tone = "accent", tabular = true }
			else ds.Badge { p.version, tabular = true },
		control = control,
	}
end

return {
	id = "ex_plugins",
	title = "Plugins",
	about = "Settings → Plugins: what's installed and what could be, searched; each switched on or off, updated or removed. And what shows while the catalog loads, when it can't be reached, and when nothing matches.",
	demo = function()
		-- A tab's plugins, each row its plugin's as the search narrows and
		-- widens; or, finding none, what it looked for.
		local function list(which: string): Node
			return ds.Settings.Each(found, function(p: any): Node
				return row(p, which == "installed")
			end, {
				key = function(p: any): string
					return p.id
				end,
			})
		end
		local function nothing(): Node
			return ds.EmptyState {
				icon = "search",
				title = function(): string
					return `Nothing matches “{query:get()}”`
				end,
				body = "Try another word, or look in the other tab.",
				size = "sm",
				action = { label = "Clear search", on_click = function() query:set("") end },
			}
		end
		local function body(key: string): Node
			if key == "loading" then
				return ds.Settings.Group {
					ds.SkeletonRow { trail = 40 },
					ds.SkeletonRow { title = 96, line = 280, trail = 40 },
					ds.SkeletonRow { title = 120, line = 200, trail = 40 },
					ds.SkeletonRow { title = 84, line = 260, trail = 40 },
				}
			elseif key == "failed" then
				return ds.Callout {
					tone = "danger",
					title = "Couldn't reach the plugin catalog",
					body = "Your installed plugins still work. Zeron will try again when you're back online.",
					action = { label = "Try again", on_click = function() shown:set("ready") end },
				}
			elseif key == "none" then
				return ds.EmptyState {
					icon = "widget",
					title = "No plugins yet",
					body = "Plugins give your agents tools: issue trackers, databases, browsers.",
					action = { label = "Browse plugins", on_click = function() shown:set("ready"); tab:set("browse") end },
				}
			end
			if string.match(key, ":nothing$") then
				return nothing()
			end
			if key == "list:browse" then
				return browse()
			end
			return list(string.match(key, "^list:(%a+)") or "installed")
		end
		return kit.stage {
			states = {
				{ value = "ready", label = "Ready" },
				{ value = "loading", label = "Loading" },
				{ value = "none", label = "Empty" },
				{ value = "failed", label = "Failed" },
			},
			state = shown,
			w = 900,
			h = 640,
			overlays = {
				ds.Dialog {
					open = removing,
					title = "Remove Sentry?",
					body = "Agents lose its tools in every project. Its settings stay, in case it comes back.",
					tone = "danger",
					confirm = { label = "Remove" },
				},
			},
			kit.settings {
				page = "plugins",
				ui.presence(function(): string
					return opened:get() or ""
				end, function(id: string): Node
				if id ~= "" then
					return listing(id)
				end
				return ds.Settings.Page {
					title = "Plugins",
					description = "Tools your agents can use, in every project in Halcyon Labs.",
					trail = ds.SearchField { value = query, placeholder = "Search plugins", size = "sm", w = 200 },
					ui.column {
						w_full = true,
						gap = "$space.4",
						ds.Tabs {
							value = tab,
							label = "Plugins",
							options = {
								{ value = "installed", label = "Installed", count = #INSTALLED },
								{ value = "browse", label = "Browse", count = #CATALOG },
							},
						},
						ui.presence(function(): string
							local s = shown:get()
							if s ~= "ready" then
								return s
							end
							-- A tab is a page of its own; within it, found or not.
							return if #found:get() > 0 then `list:{tab:get()}` else `list:{tab:get()}:nothing`
						end, body, { w_full = true, enter = { opacity = { 0, 1 }, duration = 120 }, exit = { opacity = 0, duration = 60 }, mode = "wait" }),
					},
				}
				end, { w_full = true }),
			},
		}
	end,
}
